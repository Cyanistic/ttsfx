use std::fmt::Debug;
use std::marker::PhantomData;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_with::{DeserializeAs, InspectError};
use tracing::warn;

use crate::{Result, err};

/// A partial representation of `T` where only some fields may be present.
///
/// Missing fields (not in the map) don't override the target.
/// Present fields (including null) override the target.
#[derive(Serialize, Deserialize)]
#[serde(transparent)]
pub struct Partial<T> {
    fields: Value,
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
            fields: Value::Object(Default::default()),
            _phantom: PhantomData,
        }
    }
}

impl<T> Default for Partial<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Deep-merge `overlay` into `base` when both sides are JSON objects; otherwise replace `base`.
fn merge_into(base: &mut Value, overlay: Value) {
    match (base, overlay) {
        (Value::Object(base_map), Value::Object(overlay_map)) => {
            for (k, overlay_v) in overlay_map {
                match base_map.get_mut(&k) {
                    Some(base_v) => merge_into(base_v, overlay_v),
                    None => {
                        base_map.insert(k, overlay_v);
                    }
                }
            }
        }
        (base_slot, overlay_v) => *base_slot = overlay_v,
    }
}

impl<T: Serialize + DeserializeOwned> Partial<T> {
    /// Merge this partial into `target`, returning a new `T`.
    ///
    /// - Missing fields (not in map) → keep target's value
    /// - Present fields → merge: nested objects recurse; scalars and arrays replace
    /// - Returns error if deserialization fails (e.g. null into non-nullable type)
    pub fn apply_some(&self, target: &T) -> Result<T> {
        let type_name = std::any::type_name::<T>();
        let mut root = serde_json::to_value(target).map_err(|e| {
            err!(
                Serialization,
                "failed to serialize {} for partial merge",
                type_name,
                @external: e
            )
        })?;
        merge_into(&mut root, self.fields.clone());
        serde_json::from_value(root).map_err(|e| {
            err!(
                Validation,
                "failed to deserialize {} after partial merge",
                type_name,
                @external: e
            )
        })
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
