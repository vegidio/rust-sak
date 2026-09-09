//! The attribute set attached to every record: machine, version, session, and optional location.

use std::sync::{Arc, Mutex, PoisonError, RwLock};

use opentelemetry::Key;
use opentelemetry::logs::AnyValue;

use super::Geolocation;
use super::machine_id::machine_id;

/// Attribute keys. Kept as constants so the emit path and the tests cannot drift apart.
pub(super) const VERSION: &str = "version";
pub(super) const MACHINE_ID: &str = "machine.id";
pub(super) const MACHINE_OS: &str = "machine.os";
pub(super) const MACHINE_ARCH: &str = "machine.arch";
pub(super) const SESSION_ID: &str = "session.id";
pub(super) const LOCATION_COUNTRY: &str = "location.country";
pub(super) const LOCATION_REGION: &str = "location.region";
pub(super) const LOCATION_CITY: &str = "location.city";

/// The enrichment attributes shared by every record a [`Telemetry`](super::Telemetry) emits.
///
/// The rendered attribute list is swapped wholesale rather than mutated in place, so
/// [`renew_session`](Enrichment::renew_session) and [`set_location`](Enrichment::set_location) are safe to call
/// concurrently with any number of threads emitting records.
#[derive(Debug)]
pub(super) struct Enrichment {
    /// Everything except the session id: version, machine info, and location once the lookup resolves. Guarded
    /// because the background geolocation thread appends to it after construction.
    base: Mutex<Vec<(Key, AnyValue)>>,
    /// The current session id, kept separately so it can be read back without scanning the rendered list.
    session_id: Mutex<String>,
    /// `base` plus the session id, pre-rendered so that emitting a record with no fields of its own costs only an
    /// `Arc` clone.
    rendered: RwLock<Arc<Vec<(Key, AnyValue)>>>,
}

impl Enrichment {
    /// Gathers the enrichment that can be read locally — version, service-scoped machine id, OS and architecture —
    /// and assigns an initial session id. Nothing here touches the network.
    ///
    /// `machine.os` and `machine.arch` carry Rust's own platform names (`macos`/`x86_64`), not Go's
    /// (`darwin`/`amd64`).
    pub(super) fn new(version: &str, service: &str) -> Self {
        let mut base = vec![(Key::from_static_str(VERSION), AnyValue::from(version.to_string()))];

        if let Some(id) = machine_id(service) {
            base.push((Key::from_static_str(MACHINE_ID), AnyValue::from(id)));
        }

        base.push((Key::from_static_str(MACHINE_OS), AnyValue::from(std::env::consts::OS)));
        base.push((
            Key::from_static_str(MACHINE_ARCH),
            AnyValue::from(std::env::consts::ARCH),
        ));

        let enrichment = Self {
            base: Mutex::new(base),
            session_id: Mutex::new(String::new()),
            rendered: RwLock::new(Arc::new(Vec::new())),
        };
        enrichment.renew_session();

        enrichment
    }

    /// The current attribute list. Cheap: one `Arc` clone under a briefly held read lock.
    pub(super) fn attributes(&self) -> Arc<Vec<(Key, AnyValue)>> {
        Arc::clone(&self.rendered.read().unwrap_or_else(PoisonError::into_inner))
    }

    /// The session id attached to records emitted right now.
    pub(super) fn session_id(&self) -> String {
        self.session_id.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Assigns a fresh session id to every record emitted from now on.
    pub(super) fn renew_session(&self) {
        *self.session_id.lock().unwrap_or_else(PoisonError::into_inner) = uuid::Uuid::new_v4().to_string();
        self.rerender();
    }

    /// Merges a resolved geolocation into the enrichment. Fields the service did not report are skipped.
    pub(super) fn set_location(&self, geo: &Geolocation) {
        {
            let mut base = self.base.lock().unwrap_or_else(PoisonError::into_inner);
            for (key, value) in [
                (LOCATION_COUNTRY, geo.country.as_deref()),
                (LOCATION_REGION, geo.region.as_deref()),
                (LOCATION_CITY, geo.city.as_deref()),
            ] {
                if let Some(value) = value.filter(|v| !v.is_empty()) {
                    base.push((Key::from_static_str(key), AnyValue::from(value.to_string())));
                }
            }
        }

        self.rerender();
    }

    /// Rebuilds the pre-rendered list and swaps it in.
    ///
    /// Locks are taken in the fixed order `base` → `session_id` → `rendered`, and a poisoned lock recovers the inner
    /// value rather than panicking: a telemetry module must not turn one panic into a cascade of them.
    fn rerender(&self) {
        let base = self.base.lock().unwrap_or_else(PoisonError::into_inner);
        let session_id = self.session_id.lock().unwrap_or_else(PoisonError::into_inner);

        let mut attributes = Vec::with_capacity(base.len() + 1);
        attributes.extend(base.iter().cloned());
        attributes.push((Key::from_static_str(SESSION_ID), AnyValue::from(session_id.clone())));

        *self.rendered.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(attributes);
    }

    /// The rendered attributes as a plain string map, for assertions.
    #[cfg(test)]
    pub(super) fn snapshot(&self) -> std::collections::HashMap<String, String> {
        self.attributes()
            .iter()
            .map(|(key, value)| {
                let value = match value {
                    AnyValue::String(text) => text.to_string(),
                    other => format!("{other:?}"),
                };
                (key.to_string(), value)
            })
            .collect()
    }
}
