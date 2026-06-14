use std::collections::HashMap;
use std::fmt::Debug;
use std::marker::PhantomData;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_with::{DeserializeAs, InspectError};
use tracing::warn;

use crate::{err, Result};

/// A partial representation of `T` where only some fields may be present.
///
/// Missing fields (not in the map) don't override the target.
/// Present fields (including null) override the target.
#[derive(Serialize, Deserialize)]
#[serde(transparent)]
pub struct Partial<T> {
    fields: HashMap<String, serde_json::Value>,
    #[serde(skip)]
    _phantom: PhantomData<T>,
}

impl<T> Debug for Partial<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Partial")
            .field("fields", &self.fields)
            .field("_phantom", &self._phantom)
            .finish()
    }
}

impl<T> Clone for Partial<T> {
    fn clone(&self) -> Self {
        Self {
            fields: self.fields.clone(),
            _phantom: self._phantom,
        }
    }
}

impl<T> Partial<T> {
    pub fn new() -> Self {
        Self {
            fields: HashMap::new(),
            _phantom: PhantomData,
        }
    }
}

impl<T> Default for Partial<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Serialize + DeserializeOwned> Partial<T> {
    /// Merge this partial into `target`, returning a new `T`.
    ///
    /// - Missing fields (not in map) → keep target's value
    /// - Present fields (including null) → override target's value
    /// - Returns error if deserialization fails (e.g. null into non-nullable type)
    pub fn apply_some(&self, target: &T) -> Result<T> {
        let type_name = std::any::type_name::<T>();
        let mut map = serde_json::to_value(target)
            .map_err(|e| err!(Serialization, "failed to serialize {} for partial merge", type_name, @external: e))?
            .as_object()
            .cloned()
            .ok_or_else(|| err!(Serialization, "expected object when serializing {}", type_name))?;
        for (key, value) in &self.fields {
            map.insert(key.clone(), value.clone());
        }
        serde_json::from_value(serde_json::Value::Object(map))
            .map_err(|e| err!(Validation, "failed to deserialize {} after partial merge", type_name, @external: e))
    }
}

pub struct PartialOrDefault<T>(T);

impl<'de, T, U> DeserializeAs<'de, T> for PartialOrDefault<U>
where
    U: DeserializeAs<'de, T>,
    T: Default + Serialize + DeserializeOwned,
    Partial<T>: Deserialize<'de>,
{
    fn deserialize_as<D>(deserializer: D) -> Result<T, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let partial = Partial::<T>::deserialize(deserializer)?;
        let default = T::default();
        Ok(partial
            .apply_some(&default)
            .map_err(serde::de::Error::custom)?)
    }
}

pub struct WarnOnError;

impl InspectError for WarnOnError {
    #[track_caller]
    fn inspect_error(error: impl serde::de::Error) {
        let loc = core::panic::Location::caller();
        warn!(error = %error, caller.file = loc.file(), caller.line = loc.line());
    }
}
