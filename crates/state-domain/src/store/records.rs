//! Stored source references, resolved into runtime declarations only at the catalog boundary.
use crate::{
    Error, ErrorCode, Result,
    model::{Database, Manifest},
};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use state_contract::{
    function::{EndpointSchema, FunctionDeclaration, FunctionSources},
    http::{Handler, Method, Route, RoutePattern, RouteTarget},
};
use std::collections::{BTreeMap, BTreeSet};

use crate::content::{CatalogContent, ContentHash, ContentLoader};
fn corrupt(message: &str) -> Error {
    Error::new(ErrorCode::CorruptStore, message)
}

fn intern(conn: &Connection, bytes: &[u8]) -> Result<ContentHash> {
    let digest = ContentHash::of(bytes);
    conn.execute(
        "INSERT OR IGNORE INTO objects(hash,bytes) VALUES(?,?)",
        params![digest.as_str(), bytes],
    )?;
    Ok(digest)
}
/// Resolve only the selected publication through the injected content loader.
/// Wire declarations own strings because running invocations retain snapshots.
pub(crate) struct SourceResolver<'a> {
    conn: &'a Connection,
    loader: &'a dyn ContentLoader,
}
impl<'a> SourceResolver<'a> {
    pub(crate) fn new(conn: &'a Connection, loader: &'a dyn ContentLoader) -> Self {
        Self { conn, loader }
    }
    fn text(&mut self, digest: &ContentHash) -> Result<String> {
        let bytes = self.loader.load(&CatalogContent(self.conn), digest)?;
        std::str::from_utf8(&bytes)
            .map(str::to_owned)
            .map_err(|_| corrupt("pinned source is not UTF-8"))
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SourceReferences {
    source: ContentHash,
    modules: BTreeMap<String, ContentHash>,
}
impl SourceReferences {
    fn capture(
        conn: &Connection,
        source: &str,
        modules: &BTreeMap<String, String>,
    ) -> Result<Self> {
        Ok(Self {
            source: intern(conn, source.as_bytes())?,
            modules: modules
                .iter()
                .map(|(path, source)| Ok((path.clone(), intern(conn, source.as_bytes())?)))
                .collect::<Result<_>>()?,
        })
    }
    pub(crate) fn resolve(self, resolver: &mut SourceResolver<'_>) -> Result<FunctionSources> {
        Ok(FunctionSources {
            source: resolver.text(&self.source)?,
            modules: self
                .modules
                .into_iter()
                .map(|(path, digest)| Ok((path, resolver.text(&digest)?)))
                .collect::<Result<_>>()?,
        })
    }
    fn add_references(&self, counts: &mut BTreeMap<ContentHash, i64>) -> Result<()> {
        for digest in std::iter::once(&self.source).chain(self.modules.values()) {
            add_reference(counts, digest)?;
        }
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredFunction {
    abi_version: u32,
    description: Option<String>,
    file: String,
    input_schema: Option<EndpointSchema>,
    name: String,
    output_schema: Option<EndpointSchema>,
    sources: SourceReferences,
    symbol: String,
    pub(crate) version: String,
}
impl StoredFunction {
    pub(crate) fn summary(&self) -> state_contract::function::FunctionSummary {
        state_contract::function::FunctionSummary {
            name: self.name.clone(),
            file: self.file.clone(),
            symbol: self.symbol.clone(),
            description: self.description.clone(),
            input_schema: self.input_schema.clone(),
            output_schema: self.output_schema.clone(),
            version: self.version.clone(),
        }
    }
    pub(crate) fn capture(conn: &Connection, function: &FunctionDeclaration) -> Result<Self> {
        let sources = SourceReferences::capture(conn, &function.source, &function.modules)?;
        if sources.source.as_str() != function.source_hash {
            return Err(corrupt("function source digest is inconsistent"));
        }
        Ok(Self {
            abi_version: function.abi_version,
            description: function.description.clone(),
            file: function.file.clone(),
            input_schema: function.input_schema.clone(),
            name: function.name.clone(),
            output_schema: function.output_schema.clone(),
            sources,
            symbol: function.symbol.clone(),
            version: function.version.clone(),
        })
    }
    pub(crate) fn resolve(self, resolver: &mut SourceResolver<'_>) -> Result<FunctionDeclaration> {
        let source_hash = self.sources.source.as_str().to_owned();
        let sources = self.sources.resolve(resolver)?;
        Ok(FunctionDeclaration {
            abi_version: self.abi_version,
            description: self.description,
            file: self.file,
            input_schema: self.input_schema,
            modules: sources.modules,
            name: self.name,
            output_schema: self.output_schema,
            source: sources.source,
            source_hash,
            symbol: self.symbol,
            version: self.version,
        })
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum StoredTarget {
    Function {
        file: String,
        symbol: String,
        sources: SourceReferences,
    },
    Static {
        directory: String,
        index: Option<String>,
    },
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredRoute {
    name: String,
    path: RoutePattern,
    methods: BTreeSet<Method>,
    target: StoredTarget,
}
impl StoredRoute {
    pub(crate) fn metadata(
        &self,
        namespace: &str,
        namespace_id: &str,
    ) -> state_contract::http::ApiPublished {
        state_contract::http::ApiPublished {
            namespace: namespace.into(),
            namespace_id: namespace_id.into(),
            name: self.name.clone(),
            path: self.path.clone(),
            methods: self.methods.clone(),
            urls: Vec::new(),
            target: match &self.target {
                StoredTarget::Function { file, symbol, .. } => {
                    state_contract::http::TargetInput::Function {
                        file: file.clone(),
                        symbol: symbol.clone(),
                    }
                }
                StoredTarget::Static { directory, index } => {
                    state_contract::http::TargetInput::Static {
                        directory: directory.clone(),
                        index: index.clone(),
                    }
                }
            },
        }
    }
    pub(crate) fn capture(conn: &Connection, route: &Route) -> Result<Self> {
        let target = match &route.target {
            RouteTarget::Function(handler) => StoredTarget::Function {
                file: handler.file.clone(),
                symbol: handler.symbol.clone(),
                sources: SourceReferences::capture(
                    conn,
                    &handler.sources.source,
                    &handler.sources.modules,
                )?,
            },
            RouteTarget::Static { directory, index } => StoredTarget::Static {
                directory: directory.clone(),
                index: index.clone(),
            },
        };
        Ok(Self {
            name: route.name.clone(),
            path: route.path.clone(),
            methods: route.methods.clone(),
            target,
        })
    }
    pub(crate) fn resolve(self, resolver: &mut SourceResolver<'_>) -> Result<Route> {
        let target = match self.target {
            StoredTarget::Function {
                file,
                symbol,
                sources,
            } => RouteTarget::Function(Handler {
                file,
                symbol,
                sources: sources.resolve(resolver)?,
            }),
            StoredTarget::Static { directory, index } => RouteTarget::Static { directory, index },
        };
        Ok(Route {
            name: self.name,
            path: self.path,
            methods: self.methods,
            target,
        })
    }
}

/// Compact storage representation; runtime code never consumes hash references as source text.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StoredManifest {
    routes: BTreeMap<String, StoredRoute>,
    description: Option<String>,
    files: BTreeMap<String, ContentHash>,
    databases: BTreeMap<String, Database>,
    functions: BTreeMap<String, StoredFunction>,
}
impl StoredManifest {
    pub(super) fn capture(conn: &Connection, manifest: &Manifest) -> Result<Self> {
        Ok(Self {
            routes: manifest
                .routes
                .iter()
                .map(|(name, route)| Ok((name.clone(), route.capture(conn)?)))
                .collect::<Result<_>>()?,
            description: manifest.description.clone(),
            files: manifest.files.clone(),
            databases: manifest.databases.clone(),
            functions: manifest
                .functions
                .iter()
                .map(|(name, function)| Ok((name.clone(), function.capture(conn)?)))
                .collect::<Result<_>>()?,
        })
    }
    pub(super) fn into_manifest(self) -> Manifest {
        Manifest {
            routes: self
                .routes
                .into_iter()
                .map(|(name, route)| (name, crate::model::RouteRecord::Stored(route)))
                .collect(),
            description: self.description,
            files: self.files,
            databases: self.databases,
            functions: self
                .functions
                .into_iter()
                .map(|(name, function)| (name, crate::model::FunctionRecord::Stored(function)))
                .collect(),
        }
    }
    #[cfg(test)]
    pub(super) fn resolve(self, conn: &Connection) -> Result<Manifest> {
        self.resolve_with(&mut SourceResolver::new(
            conn,
            &crate::content::DirectLoader,
        ))
    }
    #[cfg(test)]
    pub(super) fn resolve_with(self, resolver: &mut SourceResolver<'_>) -> Result<Manifest> {
        Ok(Manifest {
            routes: self
                .routes
                .into_iter()
                .map(|(name, route)| {
                    Ok((
                        name,
                        crate::model::RouteRecord::Loaded(route.resolve(resolver)?),
                    ))
                })
                .collect::<Result<_>>()?,
            description: self.description,
            files: self.files,
            databases: self.databases,
            functions: self
                .functions
                .into_iter()
                .map(|(name, function)| {
                    Ok((
                        name,
                        crate::model::FunctionRecord::Loaded(function.resolve(resolver)?),
                    ))
                })
                .collect::<Result<_>>()?,
        })
    }
    pub(super) fn record_references(&self, conn: &Connection, revision: &str) -> Result<()> {
        let mut counts = BTreeMap::new();
        for digest in self.files.values() {
            add_reference(&mut counts, digest)?;
        }
        for function in self.functions.values() {
            function.sources.add_references(&mut counts)?;
        }
        for route in self.routes.values() {
            if let StoredTarget::Function { sources, .. } = &route.target {
                sources.add_references(&mut counts)?;
            }
        }
        for (digest, count) in counts {
            conn.execute(
                "INSERT INTO revision_objects(revision_id,hash,uses) VALUES(?,?,?)",
                params![revision, digest.as_str(), count],
            )?;
        }
        Ok(())
    }
}
fn add_reference(counts: &mut BTreeMap<ContentHash, i64>, digest: &ContentHash) -> Result<()> {
    let count = counts.entry(digest.clone()).or_default();
    *count = count
        .checked_add(1)
        .ok_or_else(|| corrupt("content reference count overflow"))?;
    Ok(())
}
