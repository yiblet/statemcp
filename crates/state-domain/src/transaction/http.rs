//! Named namespace routes published using the existing transaction and manifest.
use super::Transaction;
use crate::{Error, Result, validation::name};
use state_contract::http::*;
impl Transaction {
    /// Publish a validated route and its exact validated source snapshot atomically.
    pub fn publish_api(&mut self, input: &ApiPublishInput, route: Route) -> Result<ApiPublished> {
        self.operation(|tx| {
            let key =
                tx.namespace_for_change(&input.namespace, input.expected_revision.as_deref())?;
            let route_name = name(&input.name)?;
            if route.name != route_name
                || route.path != input.path
                || route.methods != input.methods
            {
                return Err(Error::invalid(
                    "route publication differs from parsed request",
                ));
            }
            if let RouteTarget::Function(handler) = &route.target {
                tx.verify_sources(&key, &handler.file, &handler.sources)?;
            }
            for existing in tx.staged_namespace(&key)?.manifest.routes.values() {
                let existing = existing.metadata("", "");
                if existing.name != route_name
                    && route.path.ambiguous(&existing.path)
                    && (route.methods.is_empty()
                        || existing.methods.is_empty()
                        || route.methods.iter().any(|m| existing.methods.contains(m))
                        || existing.methods.iter().any(|m| route.methods.contains(m)))
                {
                    return Err(Error::invalid(
                        "ambiguous HTTP route and method registration",
                    ));
                }
            }
            let namespace = tx.staged_namespace_mut(&key)?;
            let metadata = route.metadata(&namespace.name, &namespace.id);
            namespace.manifest.routes.insert(route_name, route.into());
            namespace.dirty = true;
            Ok(metadata)
        })
    }
    /// List route metadata without loading any handler source objects.
    pub fn api_routes(
        &self,
        namespace: &str,
    ) -> Result<std::collections::BTreeMap<String, ApiPublished>> {
        let key = self.namespace_identity(namespace)?;
        let ns = self.staged_namespace(key)?;
        Ok(ns
            .manifest
            .routes
            .iter()
            .map(|(name, route)| (name.clone(), route.metadata(&ns.name, &ns.id)))
            .collect())
    }
    /// Load only the selected route's pinned handler and module content.
    pub fn api_route(&self, namespace: &str, name: &str) -> Result<Route> {
        let key = self.namespace_identity(namespace)?;
        let record = self
            .staged_namespace(key)?
            .manifest
            .routes
            .get(name)
            .ok_or_else(|| Error::new(crate::ErrorCode::NotFound, "API route does not exist"))?;
        let catalog = self.backing.connect(&crate::store::catalog::name()?)?;
        record.resolve(&mut crate::store::records::SourceResolver::new(
            &catalog,
            self.loader.as_ref(),
        ))
    }
    /// Remove a named route under the same namespace revision guard as other primitives.
    pub fn remove_api(&mut self, input: &ApiRemoveInput) -> Result<()> {
        self.operation(|tx| {
            let key =
                tx.namespace_for_change(&input.namespace, input.expected_revision.as_deref())?;
            let name = name(&input.name)?;
            let namespace = tx.staged_namespace_mut(&key)?;
            if namespace.manifest.routes.remove(&name).is_none() {
                return Err(Error::new(
                    crate::ErrorCode::NotFound,
                    "API route does not exist",
                ));
            }
            namespace.dirty = true;
            Ok(())
        })
    }
    /// Current namespace URL name, retaining stable identity for permissions.
    pub fn namespace_name(&self, namespace: &str) -> Result<&str> {
        let key = self.namespace_identity(namespace)?;
        Ok(&self.staged_namespace(key)?.name)
    }
}
