//! rmcp-generated tool dispatch. Database tools share application admission.
use crate::{AppState, Json};
use rmcp::{
    ServerHandler,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ServerConfig},
    schemars, tool, tool_handler, tool_router,
};
use serde::Deserialize;

#[derive(Clone)]
pub struct MarketServer {
    pub state: AppState,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct SliceArguments {
    pub market_product: String,
    pub delivery_point: String,
    pub availability_window: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct VoyageArguments {
    request: Json,
}

#[tool_router]
impl MarketServer {
    #[tool(
        description = "Compare indicative voyage fuel, EU ETS and FuelEU strategies, never place orders."
    )]
    async fn plan_voyage_fuel(
        &self,
        Parameters(arguments): Parameters<VoyageArguments>,
    ) -> CallToolResult {
        let now = match crate::routes::prices::captured_now(&self.state) {
            Ok(now) => now,
            Err(_) => return tool_error("plan_voyage_fuel", "Internal error"),
        };
        match super::prices::voyage(&self.state, arguments.request, now).await {
            Ok(value) => structured(value),
            Err(error) => tool_error(
                "plan_voyage_fuel",
                match error.status.as_u16() {
                    503 => "Verdaxis market tools are busy; retry shortly.",
                    422 => error.detail.as_str().unwrap_or("Internal error"),
                    _ => "Market data is temporarily unavailable.",
                },
            ),
        }
    }

    #[tool(
        description = "List supported products, delivery ports and EU scope before planning; no database access."
    )]
    fn list_markets(&self) -> CallToolResult {
        let catalog: Json =
            serde_json::from_str(include_str!("../../../../deploy/rust/mcp-catalog.json"))
                .expect("committed MCP catalog");
        structured(serde_json::json!({
            "products": catalog["products"],
            "delivery_points": catalog["delivery_points"],
        }))
    }

    #[tool(
        description = "Read anonymous bids, asks and benchmark evidence for a catalog product, port and canonical window."
    )]
    async fn get_market_slice(
        &self,
        Parameters(arguments): Parameters<SliceArguments>,
    ) -> CallToolResult {
        let now = match crate::routes::prices::captured_now(&self.state) {
            Ok(now) => now,
            Err(_) => return tool_error("get_market_slice", "Internal error"),
        };
        match super::prices::market_slice(
            &self.state,
            &arguments.market_product,
            &arguments.delivery_point,
            &arguments.availability_window,
            now,
        )
        .await
        {
            Ok(value) => structured(value),
            Err(error) => {
                let message = match error.status.as_u16() {
                    503 => "Verdaxis market tools are busy; retry shortly.",
                    422 => error.detail.as_str().unwrap_or("Internal error"),
                    _ => "Market data is temporarily unavailable.",
                };
                tool_error("get_market_slice", message)
            }
        }
    }
}

fn structured(value: Json) -> CallToolResult {
    let text = serde_json::to_string_pretty(&value).expect("JSON value serialization");
    let mut result = CallToolResult::success(vec![rmcp::model::ContentBlock::text(text)]);
    result.structured_content = Some(value);
    result
}

pub fn tool_error(tool: &str, message: &str) -> CallToolResult {
    CallToolResult::error(vec![rmcp::model::ContentBlock::text(format!(
        "Error executing tool {tool}: {message}"
    ))])
}

#[tool_handler]
impl ServerHandler for MarketServer {
    fn get_info(&self) -> ServerConfig {
        let mut info = ServerConfig::default();
        info.capabilities = rmcp::model::ServerCapabilities::builder()
            .enable_tools()
            .build();
        info.capabilities = serde_json::from_value(serde_json::json!({
            "prompts":{"listChanged":false},
            "resources":{"listChanged":false,"subscribe":false},
            "tools":{"listChanged":false}
        }))
        .expect("Python server capabilities");
        info.server_info =
            serde_json::from_value(serde_json::json!({"name":"verdaxis-market","version":""}))
                .expect("Python server identity");
        info.instructions = Some("These tools are read-only. Prices come from Verdaxis market data and caller assumptions. State the provenance of each price. Never present a demo or benchmark price as executable. Results are indicative planning estimates, not quotes or offers.".into());
        info
    }
}
