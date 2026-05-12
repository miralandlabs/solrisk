use {
    chrono::Utc,
    serde::Serialize,
    serde_json::json,
    vercel_runtime::{Body, Response, StatusCode as VercelStatusCode},
};

#[derive(Debug)]
pub enum Error {
    BadRequest(String),
    Unauthorized(String),
    NotFound(String),
    PaymentRequired(String),
    Internal(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::BadRequest(msg) => write!(f, "Bad Request: {}", msg),
            Error::Unauthorized(msg) => write!(f, "Unauthorized: {}", msg),
            Error::NotFound(msg) => write!(f, "Not Found: {}", msg),
            Error::PaymentRequired(msg) => write!(f, "Payment Required: {}", msg),
            Error::Internal(msg) => write!(f, "Internal Error: {}", msg),
        }
    }
}

impl std::error::Error for Error {}

impl Error {
    pub fn to_vercel_response(&self) -> Response<Body> {
        let date = Utc::now().format("%a, %d %b %Y %H:%M:%S GMT").to_string();
        let (status, message) = match self {
            Error::BadRequest(msg) => (VercelStatusCode::BAD_REQUEST, msg),
            Error::Unauthorized(msg) => (VercelStatusCode::UNAUTHORIZED, msg),
            Error::NotFound(msg) => (VercelStatusCode::NOT_FOUND, msg),
            Error::PaymentRequired(msg) => (VercelStatusCode::PAYMENT_REQUIRED, msg),
            Error::Internal(msg) => (VercelStatusCode::INTERNAL_SERVER_ERROR, msg),
        };

        Response::builder()
            .status(status)
            .header("Content-Type", "application/json")
            .header("Access-Control-Allow-Origin", "*")
            .header(
                "Access-Control-Allow-Methods",
                "GET, POST, PUT, PATCH, DELETE, OPTIONS",
            )
            .header(
                "Access-Control-Allow-Headers",
                "Content-Type, Authorization, Date, X-Date",
            )
            .header("X-Date", date)
            .body(Body::Text(
                json!({"error": "Error", "message": message}).to_string(),
            ))
            .unwrap()
    }
}

pub fn into_vercel_response<T: Serialize>(result: Result<T, Error>) -> Response<Body> {
    let date = Utc::now().format("%a, %d %b %Y %H:%M:%S GMT").to_string();
    match result {
        Ok(value) => Response::builder()
            .status(VercelStatusCode::OK)
            .header("Content-Type", "application/json")
            .header("Access-Control-Allow-Origin", "*")
            .header("Access-Control-Allow-Methods", "GET, POST, PUT, PATCH, DELETE, OPTIONS")
            .header("Access-Control-Allow-Headers", "Content-Type, Authorization, Date, X-Date")
            .header("X-Date", date.clone())
            .body(Body::Text(serde_json::to_string(&value).unwrap_or_else(|_| {
                json!({"error": "SerializationFailed", "message": "Failed to serialize response"}).to_string()
            })))
            .unwrap_or_else(|_| {
                Response::builder()
                    .status(VercelStatusCode::INTERNAL_SERVER_ERROR)
                    .header("Content-Type", "application/json")
                    .header("Access-Control-Allow-Origin", "*")
                    .header("Access-Control-Allow-Methods", "GET, POST, PUT, PATCH, DELETE, OPTIONS")
                    .header("Access-Control-Allow-Headers", "Content-Type, Authorization, Date, X-Date")
                    .header("X-Date", date)
                    .body(Body::Text(
                        json!({"status": 500, "error": "InternalError", "message": "Failed to build response"}).to_string()
                    ))
                    .unwrap()
            }),
        Err(error) => error.to_vercel_response(),
    }
}
