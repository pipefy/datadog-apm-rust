use hyper::{Body, Method, Request, StatusCode};

use hyper::client::connect::HttpConnector;
use rmp::encode;
use serde::Serialize;
use tokio::sync::mpsc;

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct Client {
    env: Option<String>,
    endpoint: Endpoint,
    service: String,
    http_client: hyper::Client<HttpConnector>,
    buffer_sender: mpsc::Sender<Trace>,
    buffer_size: usize,
    buffer_flush_max_interval: Duration,
}

/// Configuration settings for the client.
#[derive(Debug)]
pub struct Config {
    /// Datadog apm service name
    pub service: String,
    /// Datadog apm environment
    pub env: Option<String>,
    /// Datadog agent host/ip, defaults to `localhost`.
    pub host: String,
    /// Datadog agent port, defaults to `8196`.
    pub port: String,
    /// Client buffer queue capacity, defaults to `std::u16::MAX`.
    /// It is used for limit the amount of traces being queued in memory before drop. The client should handle send all the traces before the queue is full, you usually don't need to change this value.
    pub buffer_queue_capacity: u16,
    /// The buffer size, defaults to 200. It's the amount of traces send in a single request to datadog agent.
    pub buffer_size: u16,
    /// The buffer flush maximum interval, defaults to 200 ms. It's the maximum amount of time between buffer flushes that is the time we wait to buffer the traces before send if the buffer does not reach the buffer_size.
    pub buffer_flush_max_interval: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            env: None,
            host: "localhost".to_string(),
            port: "8126".to_string(),
            service: "".to_string(),
            buffer_queue_capacity: std::u16::MAX,
            buffer_size: 200,
            buffer_flush_max_interval: Duration::from_millis(200),
        }
    }
}

impl Client {
    const MAX_RETRIES: i32 = 5;

    pub fn new(config: Config) -> Client {
        let (buffer_sender, buffer_receiver) = mpsc::channel(config.buffer_queue_capacity as usize);

        let client = Client {
            env: config.env,
            service: config.service,
            endpoint: Endpoint::create_traces_endpoint(&*config.host, &*config.port),
            http_client: hyper::Client::new(),
            buffer_sender,
            buffer_size: config.buffer_size as usize,
            buffer_flush_max_interval: config.buffer_flush_max_interval,
        };

        spawn_consume_buffer_task(buffer_receiver, client.clone());

        client
    }

    pub fn send_trace(&mut self, trace: Trace) {
        match self.buffer_sender.try_send(trace) {
            Ok(_) => trace!("trace enqueued"),
            Err(err) => warn!("could not enqueue trace: {:?}", err),
        };
    }

    async fn send_traces(&mut self, traces: Vec<Trace>) {
        for _ in 0..Client::MAX_RETRIES {
            match self.do_send_traces(&traces).await {
                ShouldRetry::True => debug!("try sending traces again"),
                ShouldRetry::False => break,
            }
        }
    }

    async fn do_send_traces(&mut self, traces: &[Trace]) -> ShouldRetry {
        let mut should_retry = ShouldRetry::False;

        match self.http_client.request(self.build_request(traces)).await {
            Ok(resp) => {
                if resp.status().is_success() {
                    trace!("{} traces sent to datadog", traces.len());
                } else if self.should_downgrade(resp.status()) {
                    self.downgrade();
                    should_retry = ShouldRetry::True
                } else {
                    error!("error sending traces to datadog: {:?}", resp)
                }
            }
            Err(err) => error!("error sending traces to datadog: {:?}", err),
        }

        should_retry
    }

    fn build_request(&self, traces: &[Trace]) -> Request<Body> {
        let raw_traces = traces
            .iter()
            .map(|trace| map_to_raw_spans(trace, self.env.clone(), self.service.clone()))
            .collect::<Vec<Vec<RawSpan>>>();

        let trace_count = raw_traces.len();
        let payload = serialize_as_msgpack(raw_traces);

        Request::builder()
            .method(Method::POST)
            .uri(self.endpoint.endpoint())
            .header("content-type", "application/msgpack")
            .header("content-length", payload.len())
            .header("X-Datadog-Trace-Count", trace_count)
            .body(Body::from(payload))
            .unwrap()
    }

    fn should_downgrade(&self, status: StatusCode) -> bool {
        (status == 404 || status == 415) && self.endpoint.fallback.is_some()
    }

    fn downgrade(&mut self) {
        debug!(
            "trace endpoint {} didn't work, switching to fallback",
            self.endpoint.endpoint()
        );
        self.endpoint = self.endpoint.fallback().clone().unwrap();
        debug!("using trace endpoint {} now", self.endpoint.endpoint());
    }
}

#[derive(Debug, Clone)]
pub struct Trace {
    pub id: u64,
    pub spans: Vec<Span>,
    pub priority: u32,
}

#[derive(Debug, Clone)]
pub struct Span {
    pub id: u64,
    pub name: String,
    pub resource: String,
    pub parent_id: Option<u64>,
    pub start: SystemTime,
    pub duration: Duration,
    pub error: Option<ErrorInfo>,
    pub http: Option<HttpInfo>,
    pub sql: Option<SqlInfo>,
    pub r#type: String,
    pub tags: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct ErrorInfo {
    pub r#type: String,
    pub msg: String,
    pub stack: String,
}

#[derive(Debug, Clone)]
pub struct HttpInfo {
    pub url: String,
    pub status_code: String,
    pub method: String,
}

#[derive(Debug, Clone)]
pub struct SqlInfo {
    pub query: String,
    pub rows: String,
    pub db: String,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
struct RawSpan {
    service: String,
    name: String,
    resource: String,
    trace_id: u64,
    span_id: u64,
    parent_id: Option<u64>,
    start: u64,
    duration: u64,
    error: i32,
    meta: HashMap<String, String>,
    metrics: HashMap<String, f64>,
    r#type: String,
}

#[derive(Debug, Clone)]
struct Endpoint {
    endpoint: String,
    fallback: Box<Option<Endpoint>>,
}

impl Endpoint {
    fn new(endpoint: String, fallback: Option<Endpoint>) -> Self {
        Endpoint {
            endpoint,
            fallback: Box::new(fallback),
        }
    }

    pub fn create_traces_endpoint(host: &str, port: &str) -> Self {
        Endpoint::new(
            format!("http://{}:{}/{}/traces", host, port, "v0.4"),
            Some(Endpoint::new(
                format!("http://{}:{}/{}/traces", host, port, "v0.3"),
                Some(Endpoint::new(
                    format!("http://{}:{}/{}/traces", host, port, "v0.2"),
                    None,
                )),
            )),
        )
    }

    pub fn endpoint(&self) -> &str {
        &*self.endpoint
    }

    pub fn fallback(&self) -> &Option<Endpoint> {
        &self.fallback
    }
}

enum ShouldRetry {
    True,
    False,
}

fn spawn_consume_buffer_task(mut buffer_receiver: mpsc::Receiver<Trace>, mut client: Client) {
    tokio::spawn(async move {
        let mut buffer = Vec::with_capacity(client.buffer_size);
        let mut last_flushed_at = SystemTime::now();
        loop {
            match buffer_receiver.try_recv() {
                Ok(trace) => {
                    buffer.push(trace);
                }
                Err(_) => {
                    tokio::time::delay_for(client.buffer_flush_max_interval).await;
                }
            }

            if buffer.len() == client.buffer_size
                || flush_max_interval_has_passed(&buffer, &client, last_flushed_at)
            {
                client.send_traces(buffer.drain(..).collect()).await;
                last_flushed_at = SystemTime::now();
            }
        }

        fn flush_max_interval_has_passed<T>(
            buffer: &[T],
            client: &Client,
            last_flushed_at: SystemTime,
        ) -> bool {
            !buffer.is_empty()
                && SystemTime::now().duration_since(last_flushed_at).unwrap()
                    > client.buffer_flush_max_interval
        }
    });
}

fn serialize_as_msgpack(traces: Vec<Vec<RawSpan>>) -> Vec<u8> {
    // this function uses a hack over rpm_serde library,
    // because the lib does not work when the struct is wrapped in a array,
    // so it manually encode the array, and then serialize each entity in a loop

    let mut buf = Vec::new();

    encode::write_array_len(&mut buf, traces.len() as u32).unwrap();
    for spans in traces {
        encode::write_array_len(&mut buf, spans.len() as u32).unwrap();
        for span in spans {
            let mut se = rmps::Serializer::new(&mut buf).with_struct_map();
            span.serialize(&mut se).unwrap();
        }
    }
    buf
}

fn fill_meta(span: &Span, env: Option<String>) -> HashMap<String, String> {
    let mut meta = HashMap::new();
    if let Some(env) = env {
        meta.insert("env".to_string(), env);
    }

    if let Some(http) = &span.http {
        meta.insert("http.status_code".to_string(), http.status_code.clone());
        meta.insert("http.method".to_string(), http.method.clone());
        meta.insert("http.url".to_string(), http.url.clone());
    }
    if let Some(error) = &span.error {
        meta.insert("error.type".to_string(), error.r#type.clone());
        meta.insert("error.msg".to_string(), error.msg.clone());
        meta.insert("error.stack".to_string(), error.stack.clone());
    }
    if let Some(sql) = &span.sql {
        meta.insert("sql.query".to_string(), sql.query.clone());
        meta.insert("sql.rows".to_string(), sql.rows.clone());
        meta.insert("sql.db".to_string(), sql.db.clone());
    }
    for (key, value) in &span.tags {
        meta.insert(key.to_string(), value.to_string());
    }
    meta
}

fn fill_metrics(priority: u32) -> HashMap<String, f64> {
    let mut metrics = HashMap::new();
    metrics.insert("_sampling_priority_v1".to_string(), f64::from(priority));
    metrics
}

fn map_to_raw_spans(trace: &Trace, env: Option<String>, service: String) -> Vec<RawSpan> {
    let mut traces = Vec::new();
    for span in &trace.spans {
        traces.push(RawSpan {
            service: service.clone(),
            trace_id: trace.id,
            span_id: span.id,
            name: span.name.clone(),
            resource: span.resource.clone(),
            parent_id: span.parent_id,
            start: duration_to_nanos(span.start.duration_since(UNIX_EPOCH).unwrap()),
            duration: duration_to_nanos(span.duration),
            error: if span.error.is_some() { 1 } else { 0 },
            r#type: span.r#type.clone(),
            meta: fill_meta(&span, env.clone()),
            metrics: fill_metrics(trace.priority),
        });
    }
    traces
}

fn duration_to_nanos(duration: Duration) -> u64 {
    duration.as_secs() * 1_000_000_000 + duration.subsec_nanos() as u64
}

#[cfg(test)]
mod tests {
    extern crate rand;

    use super::*;

    use rand::Rng;
    use serde_json::json;

    #[tokio::test]
    #[ignore]
    async fn test_send_trace() {
        let config = Config {
            service: String::from("service_name"),
            ..Default::default()
        };
        let mut client = Client::new(config);
        let trace = a_trace();
        client.send_trace(trace);
    }

    #[tokio::test]
    async fn test_map_to_raw_spans() {
        let config = Config {
            service: String::from("service_name"),
            env: Some(String::from("staging")),
            ..Default::default()
        };
        let trace = a_trace();

        let mut expected = Vec::new();
        for span in &trace.spans {
            let mut meta: HashMap<String, String> = HashMap::new();
            meta.insert("env".to_string(), config.env.clone().unwrap());
            if let Some(http) = &span.http {
                meta.insert("http.url".to_string(), http.url.clone());
                meta.insert("http.method".to_string(), http.method.clone());
                meta.insert("http.status_code".to_string(), http.status_code.clone());
            }

            let mut metrics = HashMap::new();
            metrics.insert(
                "_sampling_priority_v1".to_string(),
                f64::from(trace.priority),
            );

            expected.push(RawSpan {
                trace_id: trace.id,
                span_id: span.id,
                parent_id: span.parent_id,
                name: span.name.clone(),
                resource: span.resource.clone(),
                service: config.service.clone(),
                r#type: span.r#type.clone(),
                start: duration_to_nanos(span.start.duration_since(UNIX_EPOCH).unwrap()),
                duration: duration_to_nanos(span.duration),
                error: 0,
                meta: meta,
                metrics: metrics,
            });
        }
        let raw_spans = map_to_raw_spans(&trace, config.env, config.service);

        assert_eq!(raw_spans, expected);
    }

    #[tokio::test]
    async fn test_message_pack_serialization() {
        let generate_span = || {
            let mut rng = rand::thread_rng();
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();
            RawSpan {
                trace_id: rng.gen::<u64>(),
                span_id: rng.gen::<u64>(),
                parent_id: None,
                name: String::from("request"),
                resource: String::from("/home"),
                service: String::from("service_name"),
                r#type: String::from("web"),
                start: now * 1_000_000_000,
                duration: 4853472865,
                error: 0,
                meta: std::collections::HashMap::new(),
                metrics: std::collections::HashMap::new(),
            }
        };

        let traces = (0..3).map(|_| vec![generate_span()]).collect::<Vec<_>>();
        let result = serialize_as_msgpack(traces.clone());

        let msgpack_as_json: serde_json::Value = rmp_serde::from_read_ref(&result).unwrap();

        // debugging utility:
        //serde_json::to_writer_pretty(std::io::stdout(), &msgpack_as_json).unwrap();

        assert_eq!(msgpack_as_json, json!(traces));
    }

    fn a_trace() -> Trace {
        let mut rng = rand::thread_rng();
        Trace {
            id: rng.gen::<u64>(),
            priority: 1,
            spans: vec![Span {
                id: rng.gen::<u64>(),
                name: String::from("request"),
                resource: String::from("/home/v3"),
                r#type: String::from("web"),
                start: SystemTime::now(),
                duration: Duration::from_secs(2),
                parent_id: None,
                http: Some(HttpInfo {
                    url: String::from("/home/v3/2?trace=true"),
                    method: String::from("GET"),
                    status_code: String::from("200"),
                }),
                error: None,
                sql: None,
                tags: HashMap::new(),
            }],
        }
    }
}
