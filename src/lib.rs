//! Unofficial Datadog APM for Rust.
//!
//! Based on [datadog documentation](https://docs.datadoghq.com/api/?lang=bash#send-traces).
//!
//! # Overview
//!
//! - built for high throughput without block the caller (using tokio channel);
//! - high configurable with sensible defaults;
//! - efficient network and resource usage: `traces` buffering + serializing to messagepack;
//! - discard traces when the buffer queue is full;
//! - low-level, so it does not automatically instrument your code;
//!
//! # Usage
//!
//! Add `datadog_apm` and `tokio` to your dependencies:
//!```not_rust
//!tokio = { version = "0.2", features = ["full"] }
//!datadog-apm = "0.2"
//!```
//!
//! - Create the client:
//! (remember to reuse the same client instead of create a new one everytime, so the buffer can work)
//! ```no_run
//! use datadog_apm::{Client, Config};
//!
//! let client = Client::new(Config {
//!     env: Some("production".to_string()),
//!     service: "my-crate".to_string(),
//!     ..Default::default()
//! });
//! ```
//!
//! - create a trace with spans:
//! (for this example there is a span for a http request and a child-span for the sql transaction)
//! ```
//! use datadog_apm::{Trace, Span, HttpInfo, ErrorInfo, SqlInfo};
//! use std::collections::HashMap;
//! use std::time::{Duration, SystemTime};
//!
//! let trace = Trace {
//!     id: 123,
//!     priority: 1,
//!     spans: vec![Span {
//!          id: 1,
//!          parent_id: None,
//!          name: "request".to_string(),
//!          resource: "GET /path".to_string(),
//!          r#type: "web".to_string(),
//!          start: SystemTime::now(),
//!          duration: Duration::from_millis(50),
//!          http: Some(HttpInfo {
//!              url: String::from("/path/2?param=true"),
//!              method: String::from("GET"),
//!              status_code: String::from("500"),
//!          }),
//!          error: Some(ErrorInfo {
//!             r#type: "unknown".to_string(),
//!             msg: "Internal error".to_string(),
//!             stack: "stack here".to_string(),
//!          }),
//!          sql: None,
//!          tags: HashMap::new(),
//!     }, Span {
//!          id: 2,
//!          parent_id: Some(1),
//!          name: "database".to_string(),
//!          resource: "select".to_string(),
//!          r#type: "db".to_string(),
//!          start: SystemTime::now(),
//!          duration: Duration::from_millis(20),
//!          http: None,
//!          error: None,
//!          sql: Some(SqlInfo {
//!             query: "select 1".to_string(),
//!             rows: "1".to_string(),
//!             db: "test".to_string(),
//!          }),
//!          tags: HashMap::new(),
//!     }]
//! };
//! ```
//!
//! - send the trace:
//! ```not_run
//! client.send_trace(trace);
//! ```
//!
//! And that's it! The trace will be buffered and sent without block the current caller.
//!
//!
//! # Config
//!
//! Check [`Config`](struct.Config.html) for all available configurations.
//!
//!
//! # Features that are not included yet: (Contributions welcome!)
//!
//! - [ ] [async-std](https://github.com/async-rs/async-std) support.
//! - [ ] [tracing](https://github.com/tokio-rs/tracing) integration.
//!
#[macro_use]
extern crate log;
extern crate rmp;
extern crate rmp_serde as rmps;
extern crate serde;

mod client;

pub use crate::client::{Client, Config, ErrorInfo, HttpInfo, Span, SqlInfo, Trace};
