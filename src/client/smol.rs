use std::{
    cmp::Ordering,
    collections::HashMap,
    future::IntoFuture,
    io,
    num::NonZeroU32,
    pin::Pin,
    task::{Context, Poll},
};

use imap_next::{
    client::{Error as NextError, Event, Options as ClientOptions},
    imap_types::{
        command::{Command, CommandBody},
        core::{AString, IString, Literal, LiteralMode, NString, QuotedChar, Tag, Vec1},
        error::ValidationError,
        extensions::{
            binary::{Literal8, LiteralOrLiteral8},
            enable::CapabilityEnable,
            sort::{SortCriterion, SortKey},
            thread::{Thread, ThreadingAlgorithm},
        },
        fetch::{MacroOrMessageDataItemNames, MessageDataItem, MessageDataItemName},
        flag::{Flag, FlagNameAttribute, StoreType},
        mailbox::{ListMailbox, Mailbox},
        response::{Code, Status, Tagged},
        search::SearchKey,
        secret::Secret,
        sequence::SequenceSet,
        IntoStatic,
    },
};
use rip_starttls::imap::smol::RipStarttls;
use smol::{
    future::FutureExt,
    io::{AsyncRead, AsyncWrite},
    net::TcpStream,
};
use thiserror::Error;
use tracing::{debug, trace, warn};

use crate::{
    stream::{self, Stream},
    tasks::{
        tasks::{
            append::{AppendTask, PostAppendCheckTask, PostAppendNoOpTask},
            appenduid::AppendUidTask,
            authenticate::AuthenticateTask,
            capability::CapabilityTask,
            check::CheckTask,
            copy::CopyTask,
            create::CreateTask,
            delete::DeleteTask,
            enable::EnableTask,
            expunge::ExpungeTask,
            fetch::{FetchFirstTask, FetchTask},
            id::IdTask,
            list::ListTask,
            login::LoginTask,
            noop::NoOpTask,
            r#move::MoveTask,
            search::SearchTask,
            select::{SelectDataUnvalidated, SelectTask},
            sort::SortTask,
            store::StoreTask,
            thread::ThreadTask,
            TaskError,
        },
        SchedulerError, SchedulerEvent, Task,
    },
};

static MAX_SEQUENCE_SIZE: u8 = u8::MAX; // 255

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("cannot upgrade client to TLS: client is already in TLS state")]
    ClientAlreadyTlsError,
    #[error("cannot do STARTTLS prefix")]
    DoStarttlsPrefixError(#[from] io::Error),
    #[error("stream error")]
    Stream(#[from] stream::Error<SchedulerError>),
    #[error("validation error")]
    Validation(#[from] ValidationError),
    #[error("cannot connect to TCP stream")]
    ConnectToTcpStreamError(#[source] io::Error),
    #[error("cannot connect to TLS stream")]
    ConnectToTlsStreamError(#[source] io::Error),

    #[cfg(feature = "async-native-tls")]
    #[error("cannot connect to native TLS stream")]
    ConnectToNativeTlsStreamError(#[source] async_native_tls::Error),
    #[cfg(feature = "async-native-tls")]
    #[error("cannot create native TLS connector")]
    CreateNativeTlsConnectorError(#[source] async_native_tls::Error),
    #[cfg(feature = "futures-rustls")]
    #[error("cannot create tokio rustls client config")]
    CreateRustlsClientConfigError(#[source] futures_rustls::rustls::Error),

    #[error("cannot receive greeting from server")]
    ReceiveGreeting(#[source] stream::Error<SchedulerError>),
    #[error("cannot resolve IMAP task")]
    ResolveTask(#[from] TaskError),
}

pub enum MaybeTlsStream {
    Plain(TcpStream),
    #[cfg(feature = "futures-rustls")]
    Rustls(futures_rustls::client::TlsStream<TcpStream>),
    #[cfg(feature = "async-native-tls")]
    NativeTls(async_native_tls::TlsStream<TcpStream>),
}

impl AsyncRead for MaybeTlsStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_read(cx, buf),
            #[cfg(feature = "futures-rustls")]
            Self::Rustls(s) => Pin::new(s).poll_read(cx, buf),
            #[cfg(feature = "async-native-tls")]
            Self::NativeTls(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for MaybeTlsStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_write(cx, buf),
            #[cfg(feature = "futures-rustls")]
            Self::Rustls(s) => Pin::new(s).poll_write(cx, buf),
            #[cfg(feature = "async-native-tls")]
            Self::NativeTls(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_flush(cx),
            #[cfg(feature = "futures-rustls")]
            Self::Rustls(s) => Pin::new(s).poll_flush(cx),
            #[cfg(feature = "async-native-tls")]
            Self::NativeTls(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_close(cx),
            #[cfg(feature = "futures-rustls")]
            Self::Rustls(s) => Pin::new(s).poll_close(cx),
            #[cfg(feature = "async-native-tls")]
            Self::NativeTls(s) => Pin::new(s).poll_close(cx),
        }
    }
}

pub struct Client {
    host: String,
    pub state: super::Client,
    pub stream: Stream<MaybeTlsStream>,
}

/// Client constructors.
///
/// This section defines 3 public constructors for [`Client`]:
/// `insecure`, `tls` and `starttls`.
impl Client {
    /// Creates an insecure client, using TCP.
    ///
    /// This constructor creates a client based on an raw
    /// [`TcpStream`], receives greeting then saves server
    /// capabilities.
    pub async fn insecure(host: impl ToString, port: u16) -> Result<Self, ClientError> {
        let mut client = Self::tcp(host, port, false).await?;

        if !client.receive_greeting().await? {
            client.refresh_capabilities().await?;
        }

        Ok(client)
    }

    /// Creates a secure client, using SSL/TLS or STARTTLS.
    ///
    /// This constructor creates an client based on a secure
    /// [`TcpStream`] wrapped into a [`TlsStream`], receives greeting
    /// then saves server capabilities.
    #[cfg(feature = "futures-rustls")]
    pub async fn rustls(
        host: impl ToString,
        port: u16,
        starttls: bool,
    ) -> Result<Self, ClientError> {
        let tcp = Self::tcp(host, port, starttls).await?;
        Self::upgrade_rustls(tcp, starttls).await
    }

    /// Creates a secure client, using SSL/TLS or STARTTLS.
    ///
    /// This constructor creates an client based on a secure
    /// [`TcpStream`] wrapped into a [`TlsStream`], receives greeting
    /// then saves server capabilities.
    #[cfg(feature = "async-native-tls")]
    pub async fn native_tls(
        host: impl ToString,
        port: u16,
        starttls: bool,
    ) -> Result<Self, ClientError> {
        let tcp = Self::tcp(host, port, starttls).await?;
        Self::upgrade_native_tls(tcp, starttls).await
    }

    /// Creates an insecure client based on a raw [`TcpStream`].
    ///
    /// This function is internally used by public constructors
    /// `insecure`, `tls` and `starttls`.
    async fn tcp(
        host: impl ToString,
        port: u16,
        discard_greeting: bool,
    ) -> Result<Self, ClientError> {
        let host = host.to_string();

        let tcp_stream = TcpStream::connect((host.as_str(), port))
            .await
            .map_err(ClientError::ConnectToTcpStreamError)?;

        let stream = Stream::new(MaybeTlsStream::Plain(tcp_stream));

        let mut opts = ClientOptions::default();
        opts.crlf_relaxed = true;
        opts.discard_greeting = discard_greeting;

        let state = super::Client::new(opts);

        Ok(Self {
            host,
            stream,
            state,
        })
    }

    /// Turns an insecure client into a secure one.
    ///
    /// The flow changes depending on the `starttls` parameter:
    ///
    /// If `true`: receives greeting, sends STARTTLS command, upgrades
    /// to TLS then force-refreshes server capabilities.
    ///
    /// If `false`: upgrades straight to TLS, receives greeting then
    /// refreshes server capabilities if needed.
    #[cfg(feature = "futures-rustls")]
    async fn upgrade_rustls(mut self, starttls: bool) -> Result<Self, ClientError> {
        use std::sync::Arc;

        use futures_rustls::{
            rustls::{pki_types::ServerName, ClientConfig},
            TlsConnector,
        };
        use rustls_platform_verifier::ConfigVerifierExt;

        let MaybeTlsStream::Plain(mut tcp_stream) = self.stream.into_inner() else {
            return Err(ClientError::ClientAlreadyTlsError);
        };

        if starttls {
            tcp_stream = RipStarttls::default()
                .do_starttls_prefix(tcp_stream)
                .await
                .map_err(ClientError::DoStarttlsPrefixError)?;
        }

        let mut config = ClientConfig::with_platform_verifier()
            .map_err(ClientError::CreateRustlsClientConfigError)?;

        // See <https://www.iana.org/assignments/tls-extensiontype-values/tls-extensiontype-values.xhtml#alpn-protocol-ids>
        config.alpn_protocols = vec![b"imap".to_vec()];

        let connector = TlsConnector::from(Arc::new(config));
        let dnsname = ServerName::try_from(self.host.clone()).unwrap();

        let tls_stream = connector
            .connect(dnsname, tcp_stream)
            .await
            .map_err(ClientError::ConnectToTlsStreamError)?;

        self.stream = Stream::new(MaybeTlsStream::Rustls(tls_stream));

        if starttls || !self.receive_greeting().await? {
            self.refresh_capabilities().await?;
        }

        Ok(self)
    }

    /// Turns an insecure client into a secure one.
    ///
    /// The flow changes depending on the `starttls` parameter:
    ///
    /// If `true`: receives greeting, sends STARTTLS command, upgrades
    /// to TLS then force-refreshes server capabilities.
    ///
    /// If `false`: upgrades straight to TLS, receives greeting then
    /// refreshes server capabilities if needed.
    #[cfg(feature = "async-native-tls")]
    async fn upgrade_native_tls(mut self, starttls: bool) -> Result<Self, ClientError> {
        use async_native_tls::TlsConnector;

        let MaybeTlsStream::Plain(mut tcp_stream) = self.stream.into_inner() else {
            return Err(ClientError::ClientAlreadyTlsError);
        };

        if starttls {
            tcp_stream = RipStarttls::default()
                .do_starttls_prefix(tcp_stream)
                .await
                .map_err(ClientError::DoStarttlsPrefixError)?;
        }

        let connector = TlsConnector::new();

        let tls_stream = connector
            .connect(&self.host, tcp_stream)
            .await
            .map_err(ClientError::ConnectToNativeTlsStreamError)?;

        self.stream = Stream::new(MaybeTlsStream::NativeTls(tls_stream));

        if starttls || !self.receive_greeting().await? {
            self.refresh_capabilities().await?;
        }

        Ok(self)
    }

    /// Receives server greeting.
    ///
    /// Returns `true` if server capabilities were found in the
    /// greeting, otherwise `false`. This boolean is internally used
    /// to determine if server capabilities need to be explicitly
    /// requested or not.
    async fn receive_greeting(&mut self) -> Result<bool, ClientError> {
        let evt = self
            .stream
            .next(&mut self.state.resolver)
            .await
            .map_err(ClientError::ReceiveGreeting)?;

        if let SchedulerEvent::GreetingReceived(greeting) = evt {
            if let Some(Code::Capability(capabilities)) = greeting.code {
                self.state.capabilities = capabilities;
                return Ok(true);
            }
        }

        Ok(false)
    }
}

/// Client low-level API.
///
/// This section defines the low-level API of the client, by exposing
/// convenient wrappers around [`Task`]s. They do not contain any
/// logic.
impl Client {
    /// Resolves the given [`Task`].
    pub async fn resolve<T: Task>(&mut self, task: T) -> Result<T::Output, ClientError> {
        Ok(self.stream.next(self.state.resolver.resolve(task)).await?)
    }

    /// Enables the given capabilities.
    pub async fn enable(
        &mut self,
        capabilities: impl IntoIterator<Item = CapabilityEnable<'_>>,
    ) -> Result<Option<Vec<CapabilityEnable<'_>>>, ClientError> {
        if !self.state.ext_enable_supported() {
            warn!("IMAP ENABLE extension not supported, skipping");
            return Ok(None);
        }

        let capabilities: Vec<_> = capabilities
            .into_iter()
            .map(IntoStatic::into_static)
            .collect();

        if capabilities.is_empty() {
            return Ok(None);
        }

        let capabilities = Vec1::try_from(capabilities).unwrap();

        Ok(self.resolve(EnableTask::new(capabilities)).await??)
    }

    /// Creates a new mailbox.
    pub async fn create(
        &mut self,
        mailbox: impl TryInto<Mailbox<'_>, Error = ValidationError>,
    ) -> Result<(), ClientError> {
        let mbox = mailbox.try_into()?.into_static();
        Ok(self.resolve(CreateTask::new(mbox)).await??)
    }

    /// Lists mailboxes.
    pub async fn list(
        &mut self,
        mailbox: impl TryInto<Mailbox<'_>, Error = ValidationError>,
        mailbox_wildcard: impl TryInto<ListMailbox<'_>, Error = ValidationError>,
    ) -> Result<
        Vec<(
            Mailbox<'static>,
            Option<QuotedChar>,
            Vec<FlagNameAttribute<'static>>,
        )>,
        ClientError,
    > {
        let mbox = mailbox.try_into()?.into_static();
        let mbox_wcard = mailbox_wildcard.try_into()?.into_static();
        Ok(self.resolve(ListTask::new(mbox, mbox_wcard)).await??)
    }

    /// Selects the given mailbox.
    pub async fn select(
        &mut self,
        mailbox: impl TryInto<Mailbox<'_>, Error = ValidationError>,
    ) -> Result<SelectDataUnvalidated, ClientError> {
        let mbox = mailbox.try_into()?.into_static();
        Ok(self.resolve(SelectTask::new(mbox)).await??)
    }

    /// Selects the given mailbox in read-only mode.
    pub async fn examine(
        &mut self,
        mailbox: impl TryInto<Mailbox<'_>, Error = ValidationError>,
    ) -> Result<SelectDataUnvalidated, ClientError> {
        let mbox = mailbox.try_into()?.into_static();
        Ok(self.resolve(SelectTask::read_only(mbox)).await??)
    }

    /// Expunges the selected mailbox.
    ///
    /// A mailbox needs to be selected before, otherwise this function
    /// will fail.
    pub async fn expunge(&mut self) -> Result<Vec<NonZeroU32>, ClientError> {
        Ok(self.resolve(ExpungeTask::new()).await??)
    }

    /// Deletes the given mailbox.
    pub async fn delete(
        &mut self,
        mailbox: impl TryInto<Mailbox<'_>, Error = ValidationError>,
    ) -> Result<(), ClientError> {
        let mbox = mailbox.try_into()?.into_static();
        Ok(self.resolve(DeleteTask::new(mbox)).await??)
    }

    /// Searches messages matching the given criteria.
    async fn _search(
        &mut self,
        criteria: impl IntoIterator<Item = SearchKey<'_>>,
        uid: bool,
    ) -> Result<Vec<NonZeroU32>, ClientError> {
        let criteria: Vec<_> = criteria.into_iter().map(IntoStatic::into_static).collect();

        let criteria = if criteria.is_empty() {
            Vec1::from(SearchKey::All)
        } else {
            Vec1::try_from(criteria).unwrap()
        };

        Ok(self
            .resolve(SearchTask::new(criteria).with_uid(uid))
            .await??)
    }

    /// Searches messages matching the given criteria.
    ///
    /// This function returns sequence numbers, if you need UID see
    /// [`Client::uid_search`].
    pub async fn search(
        &mut self,
        criteria: impl IntoIterator<Item = SearchKey<'_>>,
    ) -> Result<Vec<NonZeroU32>, ClientError> {
        self._search(criteria, false).await
    }

    /// Searches messages matching the given criteria.
    ///
    /// This function returns UIDs, if you need sequence numbers see
    /// [`Client::search`].
    pub async fn uid_search(
        &mut self,
        criteria: impl IntoIterator<Item = SearchKey<'_>>,
    ) -> Result<Vec<NonZeroU32>, ClientError> {
        self._search(criteria, true).await
    }

    /// Searches messages matching the given search criteria, sorted
    /// by the given sort criteria.
    async fn _sort(
        &mut self,
        sort_criteria: impl IntoIterator<Item = SortCriterion>,
        search_criteria: impl IntoIterator<Item = SearchKey<'_>>,
        uid: bool,
    ) -> Result<Vec<NonZeroU32>, ClientError> {
        let sort: Vec<_> = sort_criteria.into_iter().collect();
        let sort = if sort.is_empty() {
            Vec1::from(SortCriterion {
                reverse: true,
                key: SortKey::Date,
            })
        } else {
            Vec1::try_from(sort).unwrap()
        };

        let search: Vec<_> = search_criteria
            .into_iter()
            .map(IntoStatic::into_static)
            .collect();
        let search = if search.is_empty() {
            Vec1::from(SearchKey::All)
        } else {
            Vec1::try_from(search).unwrap()
        };

        Ok(self
            .resolve(SortTask::new(sort, search).with_uid(uid))
            .await??)
    }

    /// Searches messages matching the given search criteria, sorted
    /// by the given sort criteria.
    ///
    /// This function returns sequence numbers, if you need UID see
    /// [`Client::uid_sort`].
    pub async fn sort(
        &mut self,
        sort_criteria: impl IntoIterator<Item = SortCriterion>,
        search_criteria: impl IntoIterator<Item = SearchKey<'_>>,
    ) -> Result<Vec<NonZeroU32>, ClientError> {
        self._sort(sort_criteria, search_criteria, false).await
    }

    /// Searches messages matching the given search criteria, sorted
    /// by the given sort criteria.
    ///
    /// This function returns UIDs, if you need sequence numbers see
    /// [`Client::sort`].
    pub async fn uid_sort(
        &mut self,
        sort_criteria: impl IntoIterator<Item = SortCriterion>,
        search_criteria: impl IntoIterator<Item = SearchKey<'_>>,
    ) -> Result<Vec<NonZeroU32>, ClientError> {
        self._sort(sort_criteria, search_criteria, true).await
    }

    async fn _thread(
        &mut self,
        algorithm: ThreadingAlgorithm<'_>,
        search_criteria: impl IntoIterator<Item = SearchKey<'_>>,
        uid: bool,
    ) -> Result<Vec<Thread>, ClientError> {
        let alg = algorithm.into_static();

        let search: Vec<_> = search_criteria
            .into_iter()
            .map(IntoStatic::into_static)
            .collect();
        let search = if search.is_empty() {
            Vec1::from(SearchKey::All)
        } else {
            Vec1::try_from(search).unwrap()
        };

        Ok(self
            .resolve(ThreadTask::new(alg, search).with_uid(uid))
            .await??)
    }

    pub async fn thread(
        &mut self,
        algorithm: ThreadingAlgorithm<'_>,
        search_criteria: impl IntoIterator<Item = SearchKey<'_>>,
    ) -> Result<Vec<Thread>, ClientError> {
        self._thread(algorithm, search_criteria, false).await
    }

    pub async fn uid_thread(
        &mut self,
        algorithm: ThreadingAlgorithm<'_>,
        search_criteria: impl IntoIterator<Item = SearchKey<'_>>,
    ) -> Result<Vec<Thread>, ClientError> {
        self._thread(algorithm, search_criteria, true).await
    }

    async fn _store(
        &mut self,
        sequence_set: SequenceSet,
        kind: StoreType,
        flags: impl IntoIterator<Item = Flag<'_>>,
        uid: bool,
    ) -> Result<HashMap<NonZeroU32, Vec1<MessageDataItem<'static>>>, ClientError> {
        let flags: Vec<_> = flags.into_iter().map(IntoStatic::into_static).collect();

        Ok(self
            .resolve(StoreTask::new(sequence_set, kind, flags).with_uid(uid))
            .await??)
    }

    pub async fn store(
        &mut self,
        sequence_set: SequenceSet,
        kind: StoreType,
        flags: impl IntoIterator<Item = Flag<'_>>,
    ) -> Result<HashMap<NonZeroU32, Vec1<MessageDataItem<'static>>>, ClientError> {
        self._store(sequence_set, kind, flags, false).await
    }

    pub async fn uid_store(
        &mut self,
        sequence_set: SequenceSet,
        kind: StoreType,
        flags: impl IntoIterator<Item = Flag<'_>>,
    ) -> Result<HashMap<NonZeroU32, Vec1<MessageDataItem<'static>>>, ClientError> {
        self._store(sequence_set, kind, flags, true).await
    }

    async fn _silent_store(
        &mut self,
        sequence_set: SequenceSet,
        kind: StoreType,
        flags: impl IntoIterator<Item = Flag<'_>>,
        uid: bool,
    ) -> Result<(), ClientError> {
        let flags: Vec<_> = flags.into_iter().map(IntoStatic::into_static).collect();

        let task = StoreTask::new(sequence_set, kind, flags)
            .with_uid(uid)
            .silent();

        Ok(self.resolve(task).await??)
    }

    pub async fn silent_store(
        &mut self,
        sequence_set: SequenceSet,
        kind: StoreType,
        flags: impl IntoIterator<Item = Flag<'_>>,
    ) -> Result<(), ClientError> {
        self._silent_store(sequence_set, kind, flags, false).await
    }

    pub async fn uid_silent_store(
        &mut self,
        sequence_set: SequenceSet,
        kind: StoreType,
        flags: impl IntoIterator<Item = Flag<'_>>,
    ) -> Result<(), ClientError> {
        self._silent_store(sequence_set, kind, flags, true).await
    }

    pub async fn post_append_noop(&mut self) -> Result<Option<u32>, ClientError> {
        Ok(self.resolve(PostAppendNoOpTask::new()).await??)
    }

    pub async fn post_append_check(&mut self) -> Result<Option<u32>, ClientError> {
        Ok(self.resolve(PostAppendCheckTask::new()).await??)
    }

    async fn _fetch_first(
        &mut self,
        id: NonZeroU32,
        items: MacroOrMessageDataItemNames<'_>,
        uid: bool,
    ) -> Result<Vec1<MessageDataItem<'static>>, ClientError> {
        let items = items.into_static();

        Ok(self
            .resolve(FetchFirstTask::new(id, items).with_uid(uid))
            .await??)
    }

    pub async fn fetch_first(
        &mut self,
        id: NonZeroU32,
        items: MacroOrMessageDataItemNames<'_>,
    ) -> Result<Vec1<MessageDataItem<'static>>, ClientError> {
        self._fetch_first(id, items, false).await
    }

    pub async fn uid_fetch_first(
        &mut self,
        id: NonZeroU32,
        items: MacroOrMessageDataItemNames<'_>,
    ) -> Result<Vec1<MessageDataItem<'static>>, ClientError> {
        self._fetch_first(id, items, true).await
    }

    async fn _copy(
        &mut self,
        sequence_set: SequenceSet,
        mailbox: impl TryInto<Mailbox<'_>, Error = ValidationError>,
        uid: bool,
    ) -> Result<(), ClientError> {
        let mbox = mailbox.try_into()?.into_static();

        Ok(self
            .resolve(CopyTask::new(sequence_set, mbox).with_uid(uid))
            .await??)
    }

    pub async fn copy(
        &mut self,
        sequence_set: SequenceSet,
        mailbox: impl TryInto<Mailbox<'_>, Error = ValidationError>,
    ) -> Result<(), ClientError> {
        self._copy(sequence_set, mailbox, false).await
    }

    pub async fn uid_copy(
        &mut self,
        sequence_set: SequenceSet,
        mailbox: impl TryInto<Mailbox<'_>, Error = ValidationError>,
    ) -> Result<(), ClientError> {
        self._copy(sequence_set, mailbox, true).await
    }

    async fn _move(
        &mut self,
        sequence_set: SequenceSet,
        mailbox: impl TryInto<Mailbox<'_>, Error = ValidationError>,
        uid: bool,
    ) -> Result<(), ClientError> {
        let mbox = mailbox.try_into()?.into_static();

        Ok(self
            .resolve(MoveTask::new(sequence_set, mbox).with_uid(uid))
            .await??)
    }

    pub async fn r#move(
        &mut self,
        sequence_set: SequenceSet,
        mailbox: impl TryInto<Mailbox<'_>, Error = ValidationError>,
    ) -> Result<(), ClientError> {
        self._move(sequence_set, mailbox, false).await
    }

    pub async fn uid_move(
        &mut self,
        sequence_set: SequenceSet,
        mailbox: impl TryInto<Mailbox<'_>, Error = ValidationError>,
    ) -> Result<(), ClientError> {
        self._move(sequence_set, mailbox, true).await
    }

    /// Executes the `CHECK` command.
    pub async fn check(&mut self) -> Result<(), ClientError> {
        Ok(self.resolve(CheckTask::new()).await??)
    }

    /// Executes the `NOOP` command.
    pub async fn noop(&mut self) -> Result<(), ClientError> {
        Ok(self.resolve(NoOpTask::new()).await??)
    }
}

/// Client medium-level API.
///
/// This section defines the medium-level API of the client (based on
/// the low-level one), by exposing helpers that update client state
/// and use a small amount of logic (mostly conditional code depending
/// on available server capabilities).
impl Client {
    /// Fetches server capabilities, then saves them.
    pub async fn refresh_capabilities(&mut self) -> Result<(), ClientError> {
        self.state.capabilities = self.resolve(CapabilityTask::new()).await??;

        Ok(())
    }

    /// Identifies the user using the given username and password.
    pub async fn login(
        &mut self,
        username: impl TryInto<AString<'_>, Error = ValidationError>,
        password: impl TryInto<AString<'_>, Error = ValidationError>,
    ) -> Result<(), ClientError> {
        let username = username.try_into()?.into_static();
        let password = password.try_into()?.into_static();
        let login = self.resolve(LoginTask::new(username, Secret::new(password)));

        match login.await?? {
            Some(capabilities) => {
                self.state.capabilities = capabilities;
            }
            None => {
                self.refresh_capabilities().await?;
            }
        };

        Ok(())
    }

    /// Authenticates the user using the given [`AuthenticateTask`].
    ///
    /// This function also refreshes capabilities (either from the
    /// task output or from explicit request).
    async fn authenticate(&mut self, task: AuthenticateTask) -> Result<(), ClientError> {
        match self.resolve(task).await?? {
            Some(capabilities) => {
                self.state.capabilities = capabilities;
            }
            None => {
                self.refresh_capabilities().await?;
            }
        };

        Ok(())
    }

    /// Authenticates the user using the `PLAIN` mechanism.
    pub async fn authenticate_plain(
        &mut self,
        login: impl AsRef<str>,
        password: impl AsRef<str>,
    ) -> Result<(), ClientError> {
        self.authenticate(AuthenticateTask::plain(
            login.as_ref(),
            password.as_ref(),
            self.state.ext_sasl_ir_supported(),
        ))
        .await
    }

    /// Authenticates the user using the `XOAUTH2` mechanism.
    pub async fn authenticate_xoauth2(
        &mut self,
        login: impl AsRef<str>,
        token: impl AsRef<str>,
    ) -> Result<(), ClientError> {
        self.authenticate(AuthenticateTask::xoauth2(
            login.as_ref(),
            token.as_ref(),
            self.state.ext_sasl_ir_supported(),
        ))
        .await
    }

    /// Authenticates the user using the `OAUTHBEARER` mechanism.
    pub async fn authenticate_oauthbearer(
        &mut self,
        user: impl AsRef<str>,
        host: impl AsRef<str>,
        port: u16,
        token: impl AsRef<str>,
    ) -> Result<(), ClientError> {
        self.authenticate(AuthenticateTask::oauthbearer(
            user.as_ref(),
            host.as_ref(),
            port,
            token.as_ref(),
            self.state.ext_sasl_ir_supported(),
        ))
        .await
    }

    /// Exchanges client/server ids.
    ///
    /// If the server does not support the `ID` extension, this
    /// function has no effect.
    pub async fn id(
        &mut self,
        params: Option<Vec<(IString<'static>, NString<'static>)>>,
    ) -> Result<Option<Vec<(IString<'static>, NString<'static>)>>, ClientError> {
        Ok(if self.state.ext_id_supported() {
            self.resolve(IdTask::new(params)).await??
        } else {
            warn!("IMAP ID extension not supported, skipping");
            None
        })
    }

    pub async fn append(
        &mut self,
        mailbox: impl TryInto<Mailbox<'_>, Error = ValidationError>,
        flags: impl IntoIterator<Item = Flag<'_>>,
        message: impl AsRef<[u8]>,
    ) -> Result<Option<u32>, ClientError> {
        let mbox = mailbox.try_into()?.into_static();

        let flags: Vec<_> = flags.into_iter().map(IntoStatic::into_static).collect();

        let msg = to_static_literal(message, self.state.ext_binary_supported())?;

        Ok(self
            .resolve(AppendTask::new(mbox, msg).with_flags(flags))
            .await??)
    }

    pub async fn appenduid(
        &mut self,
        mailbox: impl TryInto<Mailbox<'_>, Error = ValidationError>,
        flags: impl IntoIterator<Item = Flag<'_>>,
        message: impl AsRef<[u8]>,
    ) -> Result<Option<(NonZeroU32, NonZeroU32)>, ClientError> {
        let mbox = mailbox.try_into()?.into_static();

        let flags: Vec<_> = flags.into_iter().map(IntoStatic::into_static).collect();

        let msg = to_static_literal(message, self.state.ext_binary_supported())?;

        Ok(self
            .resolve(AppendUidTask::new(mbox, msg).with_flags(flags))
            .await??)
    }
}

/// Client high-level API.
///
/// This section defines the high-level API of the client (based on
/// the low and medium ones), by exposing opinionated helpers. They
/// contain more logic, and make use of fallbacks depending on
/// available server capabilities.
impl Client {
    async fn _fetch(
        &mut self,
        sequence_set: SequenceSet,
        items: MacroOrMessageDataItemNames<'_>,
        uid: bool,
    ) -> Result<HashMap<NonZeroU32, Vec1<MessageDataItem<'static>>>, ClientError> {
        let mut items = match items {
            MacroOrMessageDataItemNames::Macro(m) => m.expand().into_static(),
            MacroOrMessageDataItemNames::MessageDataItemNames(items) => items.into_static(),
        };

        if uid {
            items.push(MessageDataItemName::Uid);
        }

        let seq_map = self
            .resolve(FetchTask::new(sequence_set, items.into()).with_uid(uid))
            .await??;

        if uid {
            let mut uid_map = HashMap::new();

            for (seq, items) in seq_map {
                let uid = items.as_ref().iter().find_map(|item| {
                    if let MessageDataItem::Uid(uid) = item {
                        Some(*uid)
                    } else {
                        None
                    }
                });

                match uid {
                    Some(uid) => {
                        uid_map.insert(uid, items);
                    }
                    None => {
                        warn!(?seq, "cannot get message uid, skipping it");
                    }
                }
            }

            Ok(uid_map)
        } else {
            Ok(seq_map)
        }
    }

    pub async fn fetch(
        &mut self,
        sequence_set: SequenceSet,
        items: MacroOrMessageDataItemNames<'_>,
    ) -> Result<HashMap<NonZeroU32, Vec1<MessageDataItem<'static>>>, ClientError> {
        self._fetch(sequence_set, items, false).await
    }

    pub async fn uid_fetch(
        &mut self,
        sequence_set: SequenceSet,
        items: MacroOrMessageDataItemNames<'_>,
    ) -> Result<HashMap<NonZeroU32, Vec1<MessageDataItem<'static>>>, ClientError> {
        self._fetch(sequence_set, items, true).await
    }

    async fn _sort_or_fallback(
        &mut self,
        sort_criteria: impl IntoIterator<Item = SortCriterion> + Clone,
        search_criteria: impl IntoIterator<Item = SearchKey<'_>>,
        fetch_items: MacroOrMessageDataItemNames<'_>,
        uid: bool,
    ) -> Result<Vec<Vec1<MessageDataItem<'static>>>, ClientError> {
        let mut fetch_items = match fetch_items {
            MacroOrMessageDataItemNames::Macro(m) => m.expand().into_static(),
            MacroOrMessageDataItemNames::MessageDataItemNames(items) => items,
        };

        if uid && !fetch_items.contains(&MessageDataItemName::Uid) {
            fetch_items.push(MessageDataItemName::Uid);
        }

        let mut fetches = HashMap::new();

        if self.state.ext_sort_supported() {
            let fetch_items = MacroOrMessageDataItemNames::MessageDataItemNames(fetch_items);
            let ids = self._sort(sort_criteria, search_criteria, uid).await?;
            let ids_chunks = ids.chunks(MAX_SEQUENCE_SIZE as usize);
            let ids_chunks_len = ids_chunks.len();

            for (n, ids) in ids_chunks.enumerate() {
                debug!(?ids, "fetching sort envelopes {}/{ids_chunks_len}", n + 1);
                let ids = SequenceSet::try_from(ids.to_vec())?;
                let items = fetch_items.clone();
                fetches.extend(self._fetch(ids, items, uid).await?);
            }

            let items = ids.into_iter().flat_map(|id| fetches.remove(&id)).collect();

            Ok(items)
        } else {
            warn!("IMAP SORT extension not supported, using fallback");

            let ids = self._search(search_criteria, uid).await?;
            let ids_chunks = ids.chunks(MAX_SEQUENCE_SIZE as usize);
            let ids_chunks_len = ids_chunks.len();

            sort_criteria
                .clone()
                .into_iter()
                .filter_map(|criterion| match criterion.key {
                    SortKey::Arrival => Some(MessageDataItemName::InternalDate),
                    SortKey::Cc => Some(MessageDataItemName::Envelope),
                    SortKey::Date => Some(MessageDataItemName::Envelope),
                    SortKey::From => Some(MessageDataItemName::Envelope),
                    SortKey::Size => Some(MessageDataItemName::Rfc822Size),
                    SortKey::Subject => Some(MessageDataItemName::Envelope),
                    SortKey::To => Some(MessageDataItemName::Envelope),
                    SortKey::DisplayFrom => None,
                    SortKey::DisplayTo => None,
                })
                .for_each(|item| {
                    if !fetch_items.contains(&item) {
                        fetch_items.push(item)
                    }
                });

            for (n, ids) in ids_chunks.enumerate() {
                debug!(?ids, "fetching search envelopes {}/{ids_chunks_len}", n + 1);
                let ids = SequenceSet::try_from(ids.to_vec())?;
                let items = fetch_items.clone();
                fetches.extend(self._fetch(ids, items.into(), uid).await?);
            }

            let mut fetches: Vec<_> = fetches.into_values().collect();

            fetches.sort_by(|a, b| {
                for criterion in sort_criteria.clone().into_iter() {
                    let mut cmp = cmp_fetch_items(&criterion.key, a, b);

                    if criterion.reverse {
                        cmp = cmp.reverse();
                    }

                    if cmp.is_ne() {
                        return cmp;
                    }
                }

                cmp_fetch_items(&SortKey::Date, a, b)
            });

            Ok(fetches)
        }
    }

    pub async fn sort_or_fallback(
        &mut self,
        sort_criteria: impl IntoIterator<Item = SortCriterion> + Clone,
        search_criteria: impl IntoIterator<Item = SearchKey<'_>>,
        fetch_items: MacroOrMessageDataItemNames<'_>,
    ) -> Result<Vec<Vec1<MessageDataItem<'static>>>, ClientError> {
        self._sort_or_fallback(sort_criteria, search_criteria, fetch_items, false)
            .await
    }

    pub async fn uid_sort_or_fallback(
        &mut self,
        sort_criteria: impl IntoIterator<Item = SortCriterion> + Clone,
        search_criteria: impl IntoIterator<Item = SearchKey<'_>>,
        fetch_items: MacroOrMessageDataItemNames<'_>,
    ) -> Result<Vec<Vec1<MessageDataItem<'static>>>, ClientError> {
        self._sort_or_fallback(sort_criteria, search_criteria, fetch_items, true)
            .await
    }

    pub async fn appenduid_or_fallback(
        &mut self,
        mailbox: impl TryInto<Mailbox<'_>, Error = ValidationError> + Clone,
        flags: impl IntoIterator<Item = Flag<'_>>,
        message: impl AsRef<[u8]>,
    ) -> Result<Option<NonZeroU32>, ClientError> {
        if self.state.ext_uidplus_supported() {
            Ok(self
                .appenduid(mailbox, flags, message)
                .await?
                .map(|(uid, _)| uid))
        } else {
            warn!("IMAP UIDPLUS extension not supported, using fallback");

            // If the mailbox is currently selected, the normal new
            // message actions SHOULD occur.  Specifically, the server
            // SHOULD notify the client immediately via an untagged
            // EXISTS response.  If the server does not do so, the
            // client MAY issue a NOOP command (or failing that, a
            // CHECK command) after one or more APPEND commands.
            //
            // <https://datatracker.ietf.org/doc/html/rfc3501#section-6.3.11>
            self.select(mailbox.clone()).await?;

            let seq = match self.append(mailbox, flags, message).await? {
                Some(seq) => seq,
                None => match self.post_append_noop().await? {
                    Some(seq) => seq,
                    None => self
                        .post_append_check()
                        .await?
                        .ok_or(ClientError::ResolveTask(TaskError::MissingData(
                            "APPENDUID: seq".into(),
                        )))?,
                },
            };

            let uid = self
                .search(Vec1::from(SearchKey::SequenceSet(seq.try_into().unwrap())))
                .await?
                .into_iter()
                .next();

            Ok(uid)
        }
    }

    async fn _move_or_fallback(
        &mut self,
        sequence_set: SequenceSet,
        mailbox: impl TryInto<Mailbox<'_>, Error = ValidationError>,
        uid: bool,
    ) -> Result<(), ClientError> {
        if self.state.ext_move_supported() {
            self._move(sequence_set, mailbox, uid).await
        } else {
            warn!("IMAP MOVE extension not supported, using fallback");
            self._copy(sequence_set.clone(), mailbox, uid).await?;
            self._silent_store(sequence_set, StoreType::Add, Some(Flag::Deleted), uid)
                .await?;
            self.expunge().await?;
            Ok(())
        }
    }

    pub async fn move_or_fallback(
        &mut self,
        sequence_set: SequenceSet,
        mailbox: impl TryInto<Mailbox<'_>, Error = ValidationError>,
    ) -> Result<(), ClientError> {
        self._move_or_fallback(sequence_set, mailbox, false).await
    }

    pub async fn uid_move_or_fallback(
        &mut self,
        sequence_set: SequenceSet,
        mailbox: impl TryInto<Mailbox<'_>, Error = ValidationError>,
    ) -> Result<(), ClientError> {
        self._move_or_fallback(sequence_set, mailbox, true).await
    }

    pub fn enqueue_idle(&mut self) -> Tag<'static> {
        let tag = self.state.resolver.scheduler.tag_generator.generate();

        self.state
            .resolver
            .scheduler
            .client_next
            .enqueue_command(Command {
                tag: tag.clone(),
                body: CommandBody::Idle,
            });

        tag.into_static()
    }

    #[tracing::instrument(name = "idle", skip_all)]
    pub async fn idle(&mut self, tag: Tag<'static>) -> Result<(), stream::Error<NextError>> {
        debug!("starting the main loop");

        loop {
            let progress = self
                .stream
                .next(&mut self.state.resolver.scheduler.client_next);
            match timeout(self.state.idle_timeout, progress).await.ok() {
                None => {
                    debug!("timed out, sending done command…");
                    self.state.resolver.scheduler.client_next.set_idle_done();
                }
                Some(Err(err)) => {
                    break Err(err);
                }
                Some(Ok(Event::IdleCommandSent { .. })) => {
                    debug!("command sent");
                }
                Some(Ok(Event::IdleAccepted { .. })) => {
                    debug!("command accepted, entering idle mode");
                }
                Some(Ok(Event::IdleRejected { status, .. })) => {
                    warn!("command rejected, aborting: {status:?}");
                    break Ok(());
                }
                Some(Ok(Event::IdleDoneSent { .. })) => {
                    debug!("done command sent");
                }
                Some(Ok(Event::DataReceived { data })) => {
                    debug!("received data, sending done command…");
                    trace!("{data:#?}");
                    self.state.resolver.scheduler.client_next.set_idle_done();
                }
                Some(Ok(Event::StatusReceived {
                    status:
                        Status::Tagged(Tagged {
                            tag: ref got_tag, ..
                        }),
                })) if *got_tag == tag => {
                    debug!("received tagged response, exiting");
                    break Ok(());
                }
                Some(event) => {
                    debug!("received unknown event, ignoring: {event:?}");
                }
            }
        }
    }

    #[tracing::instrument(name = "idle/done", skip_all)]
    pub async fn idle_done(&mut self, tag: Tag<'static>) -> Result<(), stream::Error<NextError>> {
        self.state.resolver.scheduler.client_next.set_idle_done();

        loop {
            let progress = self
                .stream
                .next(&mut self.state.resolver.scheduler.client_next)
                .await?;

            match progress {
                Event::IdleDoneSent { .. } => {
                    debug!("done command sent");
                }
                Event::StatusReceived {
                    status:
                        Status::Tagged(Tagged {
                            tag: ref got_tag, ..
                        }),
                } if *got_tag == tag => {
                    debug!("received tagged response, exiting");
                    break Ok(());
                }
                event => {
                    debug!("received unknown event, ignoring: {event:?}");
                }
            }
        }
    }
}

pub(crate) fn cmp_fetch_items(
    criterion: &SortKey,
    a: &Vec1<MessageDataItem>,
    b: &Vec1<MessageDataItem>,
) -> Ordering {
    use MessageDataItem::*;

    match &criterion {
        SortKey::Arrival => {
            let a = a.as_ref().iter().find_map(|a| {
                if let InternalDate(dt) = a {
                    Some(dt.as_ref())
                } else {
                    None
                }
            });

            let b = b.as_ref().iter().find_map(|b| {
                if let InternalDate(dt) = b {
                    Some(dt.as_ref())
                } else {
                    None
                }
            });

            a.cmp(&b)
        }
        SortKey::Date => {
            let a = a.as_ref().iter().find_map(|a| {
                if let Envelope(envelope) = a {
                    envelope.date.0.as_ref().map(AsRef::as_ref)
                } else {
                    None
                }
            });

            let b = b.as_ref().iter().find_map(|b| {
                if let Envelope(envelope) = b {
                    envelope.date.0.as_ref().map(AsRef::as_ref)
                } else {
                    None
                }
            });

            a.cmp(&b)
        }
        SortKey::Size => {
            let a = a.as_ref().iter().find_map(|a| {
                if let Rfc822Size(size) = a {
                    Some(size)
                } else {
                    None
                }
            });

            let b = b.as_ref().iter().find_map(|b| {
                if let Rfc822Size(size) = b {
                    Some(size)
                } else {
                    None
                }
            });

            a.cmp(&b)
        }
        SortKey::Subject => {
            let a = a.as_ref().iter().find_map(|a| {
                if let Envelope(envelope) = a {
                    envelope.subject.0.as_ref().map(AsRef::as_ref)
                } else {
                    None
                }
            });

            let b = b.as_ref().iter().find_map(|b| {
                if let Envelope(envelope) = b {
                    envelope.subject.0.as_ref().map(AsRef::as_ref)
                } else {
                    None
                }
            });

            a.cmp(&b)
        }
        // FIXME: Address missing Ord derive in imap-types
        SortKey::Cc | SortKey::From | SortKey::To | SortKey::DisplayFrom | SortKey::DisplayTo => {
            Ordering::Equal
        }
    }
}

pub(crate) fn to_static_literal(
    message: impl AsRef<[u8]>,
    ext_binary_supported: bool,
) -> Result<LiteralOrLiteral8<'static>, ValidationError> {
    let message = if ext_binary_supported {
        LiteralOrLiteral8::Literal8(Literal8 {
            data: message.as_ref().into(),
            mode: LiteralMode::Sync,
        })
    } else {
        warn!("IMAP BINARY extension not supported, using fallback");
        Literal::validate(message.as_ref())?;
        LiteralOrLiteral8::Literal(Literal::unvalidated(message.as_ref()))
    };

    Ok(message.into_static())
}

pub(crate) async fn timeout<R>(
    timeout: std::time::Duration,
    fut: impl IntoFuture<Output = R>,
) -> std::result::Result<R, TimedOut> {
    let ok_res = async { Ok(fut.await) };
    let err_res = async {
        smol::Timer::after(timeout).await;
        Err(TimedOut())
    };

    ok_res.or(err_res).await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("Timed Out")]
pub struct TimedOut();
