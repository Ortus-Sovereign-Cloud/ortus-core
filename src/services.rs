//! Services one app offers another, through the core.
//!
//! Apps never depend on each other. The core defines what one app may ask
//! of another as a trait (in [`crate::interfaces`]); the app that can do
//! it provides an implementation when it starts, and any app asks for it
//! by the trait. When no app provides it, or the app that does is switched
//! off, the answer is [`NotInstalled`], and the asking app carries on
//! without it: mail still delivers an invitation when there is no
//! calendar, it just cannot add it to one.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, RwLock};

use crate::app::Switches;

/// The answer when no installed, switched-on app provides a service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotInstalled {
    /// What was asked for, as the trait's name.
    pub service: &'static str,
}

impl fmt::Display for NotInstalled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "no installed app provides {}", self.service)
    }
}

impl std::error::Error for NotInstalled {}

struct Provided {
    app: &'static str,
    service: Box<dyn Any + Send + Sync>,
}

/// The services apps provide, by trait.
#[derive(Default)]
pub struct Services {
    provided: RwLock<HashMap<TypeId, Provided>>,
}

impl Services {
    pub fn new() -> Self {
        Self::default()
    }

    /// Offers `service` as the provider of `T` (a trait object type, as
    /// `dyn Calendar`). The first app to provide a service keeps it.
    pub fn provide<T: ?Sized + Send + Sync + 'static>(
        &self,
        app: &'static str,
        service: Arc<T>,
    ) -> bool {
        let mut provided = self.provided.write().unwrap_or_else(|p| p.into_inner());
        let key = TypeId::of::<Arc<T>>();
        if provided.contains_key(&key) {
            return false;
        }
        provided.insert(
            key,
            Provided {
                app,
                service: Box::new(service),
            },
        );
        true
    }

    /// The provider of `T`, when its app is installed and on.
    pub async fn get<T: ?Sized + Send + Sync + 'static>(
        &self,
        switches: &Switches,
    ) -> Result<Arc<T>, NotInstalled> {
        let not_installed = NotInstalled {
            service: std::any::type_name::<T>(),
        };
        let (app, service) = {
            let provided = self.provided.read().unwrap_or_else(|p| p.into_inner());
            let Some(entry) = provided.get(&TypeId::of::<Arc<T>>()) else {
                return Err(not_installed);
            };
            let Some(service) = entry.service.downcast_ref::<Arc<T>>() else {
                return Err(not_installed);
            };
            (entry.app, Arc::clone(service))
        };
        match switches.is_on(app).await {
            Ok(true) => Ok(service),
            _ => Err(not_installed),
        }
    }

    /// The app that provides `T`, whether or not it is on.
    pub fn provider<T: ?Sized + Send + Sync + 'static>(&self) -> Option<&'static str> {
        let provided = self.provided.read().unwrap_or_else(|p| p.into_inner());
        provided.get(&TypeId::of::<Arc<T>>()).map(|entry| entry.app)
    }
}
