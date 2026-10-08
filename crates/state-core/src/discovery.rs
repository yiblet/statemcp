//! Discovery records contain schemas and sample protocol arguments only in their JSON fields.
use crate::{
    Context, Error, Functions, Result,
    schemas::{self, RuntimeGuide, ToolDefinition},
};
use serde::Serialize;
use state_contract::discovery::*;

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum Discovery {
    Tool {
        tool: ToolDefinition,
    },
    Readme {
        title: &'static str,
        format: &'static str,
        text: &'static str,
    },
    Runtime(RuntimeGuide),
    Catalog(Catalog),
    Namespace {
        namespace: String,
        functions: Vec<EndpointContract>,
    },
}
#[derive(Debug, Serialize)]
pub struct EndpointContract {
    name: String,
    version: String,
    description: Option<String>,
    input_schema: Option<state_contract::function::EndpointSchema>,
    output_schema: Option<state_contract::function::EndpointSchema>,
}
#[derive(Debug, Serialize)]
pub struct Catalog {
    tools: Vec<ToolDefinition>,
    model: &'static str,
    runtime: &'static str,
    abi_version: u32,
    discovery: DiscoverySelectors,
    identity: &'static str,
    retention: &'static str,
    limits: DiscoveryLimits,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime_guide: Option<RuntimeGuide>,
}
#[derive(Debug, Serialize)]
struct DiscoverySelectors {
    readme: ModeSelector,
    runtime: ModeSelector,
    schemas: ModeSelector,
    operation: ToolSelector,
}
#[derive(Debug, Serialize)]
struct ModeSelector {
    mode: DiscoveryMode,
}
#[derive(Debug, Serialize)]
struct ToolSelector {
    tool: &'static str,
}
#[derive(Debug, Serialize)]
struct DiscoveryLimits {
    root_calls: usize,
    root_depth: usize,
    root_milliseconds: u128,
    runtime_json_bytes: usize,
    direct_result_bytes: usize,
}
/// Describes capabilities and published function contracts without executing Python.
#[derive(Clone)]
pub struct DiscoveryService {
    functions: Functions,
}
impl DiscoveryService {
    /// Construct discovery using the function declaration service.
    pub fn new(functions: Functions) -> Self {
        Self { functions }
    }
    /// Resolve discovery selectors with the caller's namespace authority.
    pub fn describe(&self, input: DescribeInput, context: &mut Context) -> Result<Discovery> {
        context.operation(|context| self.describe_inner(input, context))
    }
    fn describe_inner(&self, input: DescribeInput, context: &mut Context) -> Result<Discovery> {
        match input {
            DescribeInput::Tool(input) => {
                let tool = schemas::typed_tool_definitions(true)
                    .into_iter()
                    .find(|tool| tool.name == input.tool)
                    .ok_or_else(|| Error::invalid("unknown discovery tool"))?;
                Ok(Discovery::Tool { tool })
            }
            DescribeInput::Namespace(input) => {
                let namespace = context.namespace(&input.namespace)?.as_str().to_owned();
                let declarations = self.functions.contracts(&namespace, context)?;
                let functions: Vec<_> = declarations
                    .functions
                    .into_iter()
                    .filter(|declaration| {
                        input
                            .function
                            .as_ref()
                            .is_none_or(|name| name == &declaration.name)
                    })
                    .map(|declaration| EndpointContract {
                        name: declaration.name,
                        version: declaration.version,
                        description: declaration.description,
                        input_schema: declaration.input_schema,
                        output_schema: declaration.output_schema,
                    })
                    .collect();
                if input.function.is_some() && functions.is_empty() {
                    return Err(Error::new(
                        crate::ErrorCode::NotFound,
                        "endpoint contract is unavailable",
                    ));
                }
                Ok(Discovery::Namespace {
                    namespace,
                    functions,
                })
            }
            DescribeInput::Overview(input) => match input.mode.unwrap_or_default() {
                DiscoveryMode::Readme => Ok(Discovery::Readme {
                    title: "StateMCP README",
                    format: "markdown",
                    text: include_str!("../../../README.md"),
                }),
                DiscoveryMode::Runtime => Ok(Discovery::Runtime(schemas::runtime_guide())),
                mode @ (DiscoveryMode::Overview | DiscoveryMode::Full) => {
                    Ok(Discovery::Catalog(Catalog {
                        tools: schemas::typed_tool_definitions(mode == DiscoveryMode::Full),
                        model: "Namespaces contain virtual files, named SQLite databases, and published Python endpoints.",
                        runtime: "Pydantic Monty",
                        abi_version: 1,
                        discovery: DiscoverySelectors {
                            readme: ModeSelector {
                                mode: DiscoveryMode::Readme,
                            },
                            runtime: ModeSelector {
                                mode: DiscoveryMode::Runtime,
                            },
                            schemas: ModeSelector {
                                mode: DiscoveryMode::Full,
                            },
                            operation: ToolSelector { tool: "db.create" },
                        },
                        identity: "local owner; embedding principal scopes receipts only",
                        retention: "history and receipts retained until explicit maintenance; no automatic GC",
                        limits: DiscoveryLimits {
                            root_calls: context.limits().runtime.max_calls,
                            root_depth: context.limits().max_depth,
                            root_milliseconds: context.limits().runtime.max_duration.as_millis(),
                            runtime_json_bytes: context.limits().runtime.max_output_bytes,
                            direct_result_bytes: context.limits().max_result_bytes,
                        },
                        runtime_guide: (mode == DiscoveryMode::Full).then(schemas::runtime_guide),
                    }))
                }
            },
        }
    }
}
