use datadog_apm::{Client, Config};
use datadog_apm::{ErrorInfo, HttpInfo, Span, SqlInfo, Trace};
use std::collections::HashMap;
use std::time::{Duration, SystemTime};

#[tokio::main]
async fn main() {
    let client = Client::new(Config {
        env: Some("production".to_string()),
        service: "my-crate".to_string(),
        ..Default::default()
    });

    let trace = Trace {
        id: 123,
        priority: 1,
        spans: vec![
            Span {
                id: 1,
                parent_id: None,
                name: "request".to_string(),
                resource: "GET /path".to_string(),
                r#type: "web".to_string(),
                start: SystemTime::now(),
                duration: Duration::from_millis(50),
                http: Some(HttpInfo {
                    url: String::from("/path/2?param=true"),
                    method: String::from("GET"),
                    status_code: String::from("500"),
                }),
                error: Some(ErrorInfo {
                    r#type: "unknown".to_string(),
                    msg: "Internal error".to_string(),
                    stack: "stack here".to_string(),
                }),
                sql: None,
                tags: HashMap::new(),
            },
            Span {
                id: 2,
                parent_id: Some(1),
                name: "database".to_string(),
                resource: "select".to_string(),
                r#type: "db".to_string(),
                start: SystemTime::now(),
                duration: Duration::from_millis(20),
                http: None,
                error: None,
                sql: Some(SqlInfo {
                    query: "select 1".to_string(),
                    rows: "1".to_string(),
                    db: "test".to_string(),
                }),
                tags: HashMap::new(),
            },
        ],
    };

    client.send_trace(trace);

    // wait for buffer flush
    tokio::time::sleep(Duration::from_secs(2)).await;
    println!("trace sent");
}
