//! The attribute set attached to every record, span and data point: machine, session, and optional location.

use std::borrow::Cow;
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use super::Geolocation;
use super::Value;
use super::machine_id::machine_id;

/// Attribute keys. Kept as constants so the emit path and the tests cannot drift apart.
pub(super) const MACHINE_ID: &str = "machine.id";
pub(super) const MACHINE_OS: &str = "machine.os";
pub(super) const MACHINE_ARCH: &str = "machine.arch";
pub(super) const SESSION_ID: &str = "session.id";
pub(super) const LOCATION_COUNTRY: &str = "location.country";
pub(super) const LOCATION_REGION: &str = "location.region";
pub(super) const LOCATION_CITY: &str = "location.city";

/// One rendered attribute list, shared by every record that was emitted while it was current.
pub(super) type Attributes = Arc<Vec<(Cow<'static, str>, Value)>>;

/// The enrichment attributes shared by every log record, span and metric data point.
///
/// The rendered attribute list is swapped wholesale rather than mutated in place, so
/// [`renew_session`](Enrichment::renew_session) and [`set_location`](Enrichment::set_location) are safe to call
/// concurrently with any number of threads emitting.
///
/// Emitting takes an [`Arc`] clone of the current list and stores it on the record rather than copying the
/// attributes into it. That keeps the hot path to a refcount bump, and it is also what makes `renew_session`
/// correct: a record emitted just before the renewal keeps the list it was emitted under, instead of picking up
/// whatever session happened to be current when the exporter got round to it.
#[derive(Debug)]
pub(super) struct Enrichment {
    /// Everything except the session id: machine info, and location once the lookup resolves. Guarded because the
    /// background geolocation thread appends to it after construction.
    base: Mutex<Vec<(Cow<'static, str>, Value)>>,
    /// The current session id, kept separately so it can be read back without scanning the rendered list.
    session_id: Mutex<String>,
    /// `base` plus the session id, pre-rendered so that emitting costs only an `Arc` clone.
    rendered: RwLock<Attributes>,
}

impl Enrichment {
    /// Gathers the enrichment that can be read locally — the service-scoped machine id, OS and architecture — and
    /// assigns an initial session id. Nothing here touches the network.
    ///
    /// `machine.os` and `machine.arch` carry Rust's own platform names (`macos`/`x86_64`), not Go's
    /// (`darwin`/`amd64`).
    pub(super) fn new(service: &str) -> Self {
        let mut base = Vec::with_capacity(4);

        if let Some(id) = machine_id(service) {
            base.push((Cow::Borrowed(MACHINE_ID), Value::String(id)));
        }

        base.push((Cow::Borrowed(MACHINE_OS), Value::from(std::env::consts::OS)));
        base.push((Cow::Borrowed(MACHINE_ARCH), Value::from(std::env::consts::ARCH)));

        let enrichment = Self {
            base: Mutex::new(base),
            session_id: Mutex::new(String::new()),
            rendered: RwLock::new(Arc::new(Vec::new())),
        };
        enrichment.renew_session();

        enrichment
    }

    /// The current attribute list. Cheap: one `Arc` clone under a briefly held read lock.
    pub(super) fn attributes(&self) -> Attributes {
        Arc::clone(&self.rendered.read().unwrap_or_else(PoisonError::into_inner))
    }

    /// The service-scoped machine id every record carries, or `None` when the host id could not be read.
    pub(super) fn machine_id(&self) -> Option<String> {
        let base = self.base.lock().unwrap_or_else(PoisonError::into_inner);

        base.iter().find_map(|(key, value)| match value {
            Value::String(id) if key == MACHINE_ID => Some(id.clone()),
            _ => None,
        })
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
    ///
    /// Any location already recorded is **replaced**, not added to. Only one lookup is started today, so this cannot
    /// yet fire — but appending would silently put two `location.country` attributes on every record, and an
    /// invariant held only by there being a single caller is one refactor away from being untrue.
    pub(super) fn set_location(&self, geo: &Geolocation) {
        {
            let mut base = self.base.lock().unwrap_or_else(PoisonError::into_inner);
            base.retain(|(key, _)| !matches!(key.as_ref(), LOCATION_COUNTRY | LOCATION_REGION | LOCATION_CITY));

            for (key, value) in [
                (LOCATION_COUNTRY, geo.country.as_deref()),
                (LOCATION_REGION, geo.region.as_deref()),
                (LOCATION_CITY, geo.city.as_deref()),
            ] {
                if let Some(value) = value.filter(|v| !v.is_empty()) {
                    base.push((Cow::Borrowed(key), Value::from(value)));
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
        attributes.push((Cow::Borrowed(SESSION_ID), Value::String(session_id.clone())));

        *self.rendered.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(attributes);
    }

    /// The rendered attributes as a plain string map, for assertions.
    #[cfg(test)]
    pub(super) fn snapshot(&self) -> std::collections::HashMap<String, String> {
        self.attributes()
            .iter()
            .map(|(key, value)| {
                let value = match value {
                    Value::String(text) => text.clone(),
                    other => format!("{other:?}"),
                };
                (key.to_string(), value)
            })
            .collect()
    }
}
