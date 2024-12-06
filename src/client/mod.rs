pub mod tokio;

use std::time::Duration;

use imap_next::{
    client::Options as ClientOptions,
    imap_types::{auth::AuthMechanism, core::Vec1, response::Capability},
};

use crate::tasks::resolver::Resolver;

pub struct Client {
    resolver: Resolver,
    capabilities: Vec1<Capability<'static>>,
    idle_timeout: Duration,
}

impl Client {
    pub fn new(opts: ClientOptions) -> Self {
        let client = imap_next::client::Client::new(opts);
        let resolver = Resolver::new(client);

        Self {
            resolver,
            capabilities: Vec1::from(Capability::Imap4Rev1),
            idle_timeout: Duration::from_secs(5 * 60), // 5 min
        }
    }

    pub fn get_idle_timeout(&self) -> &Duration {
        &self.idle_timeout
    }

    pub fn set_idle_timeout(&mut self, timeout: Duration) {
        self.idle_timeout = timeout;
    }

    pub fn set_some_idle_timeout(&mut self, timeout: Option<Duration>) {
        if let Some(timeout) = timeout {
            self.set_idle_timeout(timeout)
        }
    }

    pub fn with_idle_timeout(mut self, timeout: Duration) -> Self {
        self.set_idle_timeout(timeout);
        self
    }

    pub fn with_some_idle_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.set_some_idle_timeout(timeout);
        self
    }

    /// Returns the server capabilities.
    ///
    /// This function does not *fetch* capabilities from server, it
    /// just returns capabilities saved during the creation of this
    /// client (using [`Client::insecure`], [`Client::tls`] or
    /// [`Client::starttls`]).
    pub fn capabilities(&self) -> &Vec1<Capability<'static>> {
        &self.capabilities
    }

    /// Returns the server capabilities, as an iterator.
    ///
    /// Same as [`Client::capabilities`], but just returns an iterator
    /// instead.
    pub fn capabilities_iter(&self) -> impl Iterator<Item = &Capability<'static>> + '_ {
        self.capabilities().as_ref().iter()
    }

    /// Returns supported authentication mechanisms, as an iterator.
    pub fn supported_auth_mechanisms(&self) -> impl Iterator<Item = &AuthMechanism<'static>> + '_ {
        self.capabilities_iter().filter_map(|capability| {
            if let Capability::Auth(mechanism) = capability {
                Some(mechanism)
            } else {
                None
            }
        })
    }

    /// Returns `true` if the given authentication mechanism is
    /// supported by the server.
    pub fn supports_auth_mechanism(&self, mechanism: AuthMechanism<'static>) -> bool {
        self.capabilities_iter().any(|capability| {
            if let Capability::Auth(m) = capability {
                m == &mechanism
            } else {
                false
            }
        })
    }

    /// Returns `true` if `LOGIN` is supported by the server.
    pub fn login_supported(&self) -> bool {
        !self
            .capabilities_iter()
            .any(|c| matches!(c, Capability::LoginDisabled))
    }

    /// Returns `true` if the `ENABLE` extension is supported by the
    /// server.
    pub fn ext_enable_supported(&self) -> bool {
        self.capabilities_iter()
            .any(|c| matches!(c, Capability::Enable))
    }

    /// Returns `true` if the `SASL-IR` extension is supported by the
    /// server.
    pub fn ext_sasl_ir_supported(&self) -> bool {
        self.capabilities_iter()
            .any(|c| matches!(c, Capability::SaslIr))
    }

    /// Returns `true` if the `ID` extension is supported by the
    /// server.
    pub fn ext_id_supported(&self) -> bool {
        self.capabilities_iter()
            .any(|c| matches!(c, Capability::Id))
    }

    /// Returns `true` if the `UIDPLUS` extension is supported by the
    /// server.
    pub fn ext_uidplus_supported(&self) -> bool {
        self.capabilities_iter()
            .any(|c| matches!(c, Capability::UidPlus))
    }

    /// Returns `true` if the `SORT` extension is supported by the
    /// server.
    pub fn ext_sort_supported(&self) -> bool {
        self.capabilities_iter()
            .any(|c| matches!(c, Capability::Sort(_)))
    }

    /// Returns `true` if the `THREAD` extension is supported by the
    /// server.
    pub fn ext_thread_supported(&self) -> bool {
        self.capabilities_iter()
            .any(|c| matches!(c, Capability::Thread(_)))
    }

    /// Returns `true` if the `IDLE` extension is supported by the
    /// server.
    pub fn ext_idle_supported(&self) -> bool {
        self.capabilities_iter()
            .any(|c| matches!(c, Capability::Idle))
    }

    /// Returns `true` if the `BINARY` extension is supported by the
    /// server.
    pub fn ext_binary_supported(&self) -> bool {
        self.capabilities_iter()
            .any(|c| matches!(c, Capability::Binary))
    }

    /// Returns `true` if the `MOVE` extension is supported by the
    /// server.
    pub fn ext_move_supported(&self) -> bool {
        self.capabilities_iter()
            .any(|c| matches!(c, Capability::Move))
    }
}
