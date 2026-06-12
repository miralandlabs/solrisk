use {
    solrisk::{
        api, config::Config, init::init_tracing, route_handler::run_server, state::AppState,
        well_known,
    },
    std::{future::Future, pin::Pin, sync::Arc},
    vercel_runtime::{Body, Response, StatusCode as VercelStatusCode},
};

fn cors_options() -> Response<Body> {
    Response::builder()
        .status(VercelStatusCode::NO_CONTENT)
        .header("Access-Control-Allow-Origin", "*")
        .header("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
        .header("Access-Control-Allow-Headers", api::RISK_CORS_ALLOW_HEADERS)
        .header("Access-Control-Max-Age", "86400")
        .body(Body::Empty)
        .unwrap()
}

fn base_url_from_headers(headers: &http::HeaderMap) -> String {
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get("host"))
        .and_then(|h| h.to_str().ok())
        .unwrap_or("solrisk.signer-payer.me");
    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("https");
    format!("{proto}://{host}")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    init_tracing();

    let config = Config::from_env()?;
    let state = AppState::new(&config)?;
    let shared_state = Arc::new(state);

    let routes = |headers: http::HeaderMap,
                  method: http::Method,
                  path: String,
                  query: String,
                  _body: Body,
                  state: Arc<AppState>|
     -> Pin<Box<dyn Future<Output = Response<Body>> + Send>> {
        Box::pin(async move {
            let effective_method = if method == http::Method::HEAD {
                http::Method::GET
            } else {
                method
            };
            match (&effective_method, path.as_str()) {
                (&http::Method::GET, "/") => {
                    let accept = headers
                        .get("Accept")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("");
                    if accept.contains("text/markdown") {
                        Response::builder()
                            .status(VercelStatusCode::OK)
                            .header("Content-Type", "text/markdown; charset=utf-8")
                            .header("Link", "</openapi.json>; rel=\"service-desc\"")
                            .body(Body::Text(
                                include_str!("../../public/agent-integration.md").to_string(),
                            ))
                            .unwrap()
                    } else {
                        Response::builder()
                            .status(VercelStatusCode::OK)
                            .header("Content-Type", "text/html")
                            .header("Link", "</openapi.json>; rel=\"service-desc\"")
                            .body(Body::Text(
                                include_str!("../../public/index.html").to_string(),
                            ))
                            .unwrap()
                    }
                }

                (&http::Method::GET, "/health") => api::handle_health(state).await,

                (&http::Method::GET, "/agent-integration.md") => Response::builder()
                    .status(VercelStatusCode::OK)
                    .header("Content-Type", "text/markdown; charset=utf-8")
                    .body(Body::Text(
                        include_str!("../../public/agent-integration.md").to_string(),
                    ))
                    .unwrap(),

                (&http::Method::GET, "/openapi.json") => Response::builder()
                    .status(VercelStatusCode::OK)
                    .header("Content-Type", "application/json; charset=utf-8")
                    .body(Body::Text(
                        include_str!("../../public/openapi.json").to_string(),
                    ))
                    .unwrap(),

                (&http::Method::GET, "/.well-known/x402-resources.json") => {
                    let base = base_url_from_headers(&headers);
                    let manifest = well_known::build_x402_resources(&state, &base).await;
                    Response::builder()
                        .status(VercelStatusCode::OK)
                        .header("Content-Type", "application/json; charset=utf-8")
                        .body(Body::Text(manifest.to_string()))
                        .unwrap()
                }

                (&http::Method::OPTIONS, "/api/v1/wallet-risk")
                | (&http::Method::OPTIONS, "/api/v1/token-risk")
                | (&http::Method::OPTIONS, "/api/v1/tx-risk")
                | (&http::Method::OPTIONS, "/api/v1/subscribe")
                | (&http::Method::OPTIONS, "/api/v1/subscribe/info") => cors_options(),

                (&http::Method::GET, "/api/v1/wallet-risk") => {
                    api::handle_wallet_risk(&headers, &query, state).await
                }

                (&http::Method::GET, "/api/v1/token-risk") => {
                    api::handle_token_risk(&headers, &query, state).await
                }

                (&http::Method::GET, "/api/v1/tx-risk") => {
                    api::handle_tx_risk(&headers, &query, state).await
                }

                (&http::Method::GET, "/api/v1/subscribe/info") => {
                    api::handle_subscribe_info(state).await
                }

                (&http::Method::POST, "/api/v1/subscribe") => {
                    api::handle_subscribe(&headers, &query, state).await
                }

                _ => Response::builder()
                    .status(VercelStatusCode::NOT_FOUND)
                    .body(Body::Text("Not found".to_string()))
                    .unwrap(),
            }
        })
    };

    run_server(shared_state, routes).await
}
