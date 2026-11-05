//! Print the OpenAPI document of API v1 (`/api/v1/openapi.json`) without a database or a running server.
//!
//!     cargo run --quiet --bin openapi > ../web/src/api/openapi.json

fn main() {
    let doc = anistock::api::v1::openapi::document();
    println!("{}", serde_json::to_string_pretty(&doc).expect("json"));
}
