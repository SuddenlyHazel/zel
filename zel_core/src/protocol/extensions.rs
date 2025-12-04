use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;

/// A type-safe, thread-safe container for storing arbitrary types.
///
/// This structure allows storing multiple values of different types in a single container,
/// with type-safe retrieval. Values are stored wrapped in Arc for efficient cloning.
///
/// Extensions are used at three levels in Zel RPC:
/// - Server Extensions: Shared across all connections (e.g., database pools)
/// - Connection Extensions: Scoped to a single connection (e.g., authentication sessions)  
/// - Request Extensions: Unique per request (e.g., trace IDs)
#[derive(Clone, Default)]
pub struct Extensions {
    map: Arc<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>,
}

impl Extensions {
    /// Create a new, empty Extensions container.
    pub fn new() -> Self {
        Self {
            map: Arc::new(HashMap::new()),
        }
    }

    /// Add a value to the extensions, returning a new Extensions instance.
    ///
    /// This method creates a new Extensions instance with the added value,
    /// leaving the original unchanged. This is useful for building up
    /// extensions in a builder-style pattern.
    ///
    /// Values are stored by their TypeId, so each type can only be stored once.
    /// Adding a value of a type that already exists will replace the previous value.
    ///
    /// CRITICAL: This method correctly clones the HashMap contents, not just the Arc pointer.
    /// Using (*self.map).clone() ensures we get a new HashMap with cloned Arc pointers
    /// to the values, rather than just cloning the outer Arc.
    pub fn with<T: Send + Sync + 'static>(self, value: T) -> Self {
        let mut new_map = (*self.map).clone();
        new_map.insert(TypeId::of::<T>(), Arc::new(value));
        Self {
            map: Arc::new(new_map),
        }
    }

    /// Retrieve a value from the extensions by type.
    ///
    /// Returns None if no value of the specified type exists.
    pub fn get<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.map.get(&TypeId::of::<T>()).and_then(|boxed| {
            let cloned = Arc::clone(boxed);
            cloned.downcast::<T>().ok()
        })
    }

    /// Check if a value of the specified type exists in the extensions.
    pub fn contains<T: Send + Sync + 'static>(&self) -> bool {
        self.map.contains_key(&TypeId::of::<T>())
    }

    /// Retrieve a value from the extensions, or provide a default if it doesn't exist.
    pub fn get_or_else<T, F>(&self, default: F) -> Arc<T>
    where
        T: Send + Sync + 'static,
        F: FnOnce() -> T,
    {
        self.get::<T>().unwrap_or_else(|| Arc::new(default()))
    }
}

impl std::fmt::Debug for Extensions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Extensions")
            .field("count", &self.map.len())
            .finish()
    }
}
