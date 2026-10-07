//! Backend-only French liaison lookup. No remote pronunciation data is retained.
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    sync::Mutex,
    time::{Duration, Instant},
};

pub const LOOKUP_FAILED: &str = "FRENCH_LIAISON_LOOKUP_FAILED";
pub const ENDPOINT: &str = "https://api.lectura.world/g2p/analyser";
pub const MAX_TOKENS: usize = 128;
const MAX_REQUEST_BYTES: usize = 16 * 1024;
const MAX_RESPONSE_BYTES: u64 = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub enum Label {
    #[serde(rename = "none")]
    None,
    Lz,
    Lt,
    Ln,
    Lr,
    Lp,
}
impl Label {
    pub fn phone(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Lz => Some("fr/z"),
            Self::Lt => Some("fr/t"),
            Self::Ln => Some("fr/n"),
            Self::Lr => Some("fr/r"),
            Self::Lp => Some("fr/p"),
        }
    }
}

/// Deliberately contains no provider error, URL, response body or lyric text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    Unavailable,
    Timeout,
    Network,
    Http,
    InvalidResponse,
    Limit,
}
impl Failure {
    pub fn message(self) -> &'static str {
        match self {
            Self::Unavailable => {
                "Lectura liaison client unavailable; local liaison rules retained."
            }
            Self::Timeout => "Lectura liaison lookup timed out; local liaison rules retained.",
            Self::Network => "Lectura liaison network lookup failed; local liaison rules retained.",
            Self::Http => "Lectura liaison HTTP lookup failed; local liaison rules retained.",
            Self::InvalidResponse => {
                "Lectura liaison response invalid; local liaison rules retained."
            }
            Self::Limit => {
                "Lectura liaison phrase exceeds request limits; local liaison rules retained."
            }
        }
    }
}

pub trait LiaisonPredictor: Send + Sync {
    fn predict(&self, tokens: &[String]) -> Result<Vec<Label>, Failure>;
    fn predict_with_timeout(
        &self,
        tokens: &[String],
        _timeout: Duration,
    ) -> Result<Vec<Label>, Failure> {
        self.predict(tokens)
    }
}

struct Budget {
    remaining: Duration,
    available: bool,
}
impl Budget {
    fn record(&mut self, elapsed: Duration, result: &Result<Vec<Label>, Failure>) {
        self.remaining = self.remaining.saturating_sub(elapsed);
        if matches!(
            result,
            Err(Failure::Unavailable | Failure::Timeout | Failure::Network | Failure::Http)
        ) {
            self.available = false;
        }
    }
}

/// One budget/circuit per analysis batch; it never survives into export.
pub(crate) struct BudgetedPredictor<'a> {
    inner: &'a dyn LiaisonPredictor,
    budget: Mutex<Budget>,
}
impl<'a> BudgetedPredictor<'a> {
    pub(crate) fn new(inner: &'a dyn LiaisonPredictor, limit: Duration) -> Self {
        Self {
            inner,
            budget: Mutex::new(Budget {
                remaining: limit,
                available: true,
            }),
        }
    }
}
impl LiaisonPredictor for BudgetedPredictor<'_> {
    fn predict(&self, tokens: &[String]) -> Result<Vec<Label>, Failure> {
        let mut budget = self.budget.lock().map_err(|_| Failure::Unavailable)?;
        if !budget.available || budget.remaining.is_zero() {
            return Err(Failure::Unavailable);
        }
        let started = Instant::now();
        let result = self
            .inner
            .predict_with_timeout(tokens, budget.remaining.min(Duration::from_secs(4)));
        budget.record(started.elapsed(), &result);
        result
    }
}

pub struct Client {
    http: Result<reqwest::blocking::Client, Failure>,
    endpoint: String,
    token: Option<String>,
}
impl Client {
    pub fn production() -> Self {
        Self::new(
            ENDPOINT,
            std::env::var("VERSE_LECTURA_BEARER_TOKEN").ok(),
            Duration::from_secs(4),
        )
    }

    fn new(endpoint: &str, token: Option<String>, timeout: Duration) -> Self {
        Self {
            http: reqwest::blocking::Client::builder()
                .connect_timeout(Duration::from_secs(2).min(timeout))
                .timeout(timeout)
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .build()
                .map_err(|_| Failure::Unavailable),
            endpoint: endpoint.into(),
            token,
        }
    }
}

#[derive(Serialize)]
struct Request<'a> {
    tokens: &'a [String],
}
#[derive(Deserialize)]
struct Response {
    tokens: Vec<String>,
    liaison: Vec<Label>,
    // g2p/POS/morphology are intentionally neither decoded nor used.
}

impl LiaisonPredictor for Client {
    fn predict(&self, tokens: &[String]) -> Result<Vec<Label>, Failure> {
        // Keep a client-specific test timeout unless a batch tightens it.
        self.lookup(tokens, None)
    }
    fn predict_with_timeout(
        &self,
        tokens: &[String],
        timeout: Duration,
    ) -> Result<Vec<Label>, Failure> {
        self.lookup(tokens, Some(timeout))
    }
}
impl Client {
    fn lookup(&self, tokens: &[String], timeout: Option<Duration>) -> Result<Vec<Label>, Failure> {
        if tokens.len() < 2 || tokens.len() > MAX_TOKENS {
            return Err(Failure::Limit);
        }
        let body = serde_json::to_vec(&Request { tokens }).map_err(|_| Failure::Limit)?;
        if body.len() > MAX_REQUEST_BYTES {
            return Err(Failure::Limit);
        }
        let http = self.http.as_ref().map_err(|e| *e)?;
        let mut request = http
            .post(&self.endpoint)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body);
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
        }
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        let response = request.send().map_err(|e| {
            if e.is_timeout() {
                Failure::Timeout
            } else {
                Failure::Network
            }
        })?;
        if !response.status().is_success() {
            return Err(Failure::Http);
        }
        if response
            .content_length()
            .is_some_and(|n| n > MAX_RESPONSE_BYTES)
        {
            return Err(Failure::InvalidResponse);
        }
        let mut bytes = Vec::new();
        response
            .take(MAX_RESPONSE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Failure::Network)?;
        if bytes.len() as u64 > MAX_RESPONSE_BYTES {
            return Err(Failure::InvalidResponse);
        }
        let response: Response =
            serde_json::from_slice(&bytes).map_err(|_| Failure::InvalidResponse)?;
        if response.tokens != tokens || response.liaison.len() != tokens.len() {
            return Err(Failure::InvalidResponse);
        }
        Ok(response.liaison)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, net::TcpListener, sync::mpsc, thread};

    fn server(
        status: &str,
        body: &str,
        delay: Duration,
    ) -> (String, mpsc::Receiver<String>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/g2p/analyser", listener.local_addr().unwrap());
        let redirect = if status.starts_with("302") {
            "Location: http://127.0.0.1:1/redirect\r\n"
        } else {
            ""
        };
        let reply = format!("HTTP/1.1 {status}\r\n{redirect}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        let (tx, rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0; 1024];
            loop {
                let n = socket.read(&mut buffer).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buffer[..n]);
                let text = String::from_utf8_lossy(&bytes);
                if let Some((headers, body)) = text.split_once("\r\n\r\n") {
                    let length: usize = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .map(str::to_owned)
                        })
                        .unwrap()
                        .parse()
                        .unwrap();
                    if body.len() >= length {
                        break;
                    }
                }
            }
            tx.send(String::from_utf8(bytes).unwrap()).unwrap();
            thread::sleep(delay);
            let _ = socket.write_all(reply.as_bytes());
        });
        (endpoint, rx, worker)
    }

    fn tokens() -> Vec<String> {
        vec!["tes".into(), "yeux".into()]
    }

    #[test]
    fn batch_budget_clamps_remaining_timeout_and_stops_after_service_failure() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Capture {
            calls: AtomicUsize,
            failure: Option<Failure>,
            timeouts: Mutex<Vec<Duration>>,
        }
        impl LiaisonPredictor for Capture {
            fn predict(&self, _: &[String]) -> Result<Vec<Label>, Failure> {
                unreachable!()
            }
            fn predict_with_timeout(
                &self,
                tokens: &[String],
                timeout: Duration,
            ) -> Result<Vec<Label>, Failure> {
                self.calls.fetch_add(1, Ordering::Relaxed);
                self.timeouts.lock().unwrap().push(timeout);
                self.failure
                    .map_or_else(|| Ok(vec![Label::None; tokens.len()]), Err)
            }
        }
        let inner = Capture {
            calls: AtomicUsize::new(0),
            failure: None,
            timeouts: Mutex::new(vec![]),
        };
        let predictor = BudgetedPredictor::new(&inner, Duration::from_secs(8));
        // Inject elapsed charges rather than sleeping or depending on scheduler timing.
        predictor
            .budget
            .lock()
            .unwrap()
            .record(Duration::from_secs(7), &Ok(vec![]));
        assert!(predictor.predict(&tokens()).is_ok());
        assert_eq!(
            *inner.timeouts.lock().unwrap(),
            vec![Duration::from_secs(1)]
        );
        predictor
            .budget
            .lock()
            .unwrap()
            .record(Duration::from_secs(1), &Ok(vec![]));
        assert_eq!(predictor.predict(&tokens()), Err(Failure::Unavailable));
        assert_eq!(inner.calls.load(Ordering::Relaxed), 1);
        for failure in [
            Failure::Unavailable,
            Failure::Timeout,
            Failure::Network,
            Failure::Http,
        ] {
            let inner = Capture {
                calls: AtomicUsize::new(0),
                failure: Some(failure),
                timeouts: Mutex::new(vec![]),
            };
            let predictor = BudgetedPredictor::new(&inner, Duration::from_secs(8));
            assert_eq!(predictor.predict(&tokens()), Err(failure));
            for _ in 0..3 {
                assert_eq!(predictor.predict(&tokens()), Err(Failure::Unavailable));
            }
            assert_eq!(inner.calls.load(Ordering::Relaxed), 1);
        }
    }

    #[test]
    fn oversized_response_without_length_is_rejected_while_peer_remains_open() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/g2p/analyser", listener.local_addr().unwrap());
        let (close_tx, close_rx) = mpsc::channel();
        let (ended_tx, ended_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = [0; 2048];
            assert!(socket.read(&mut request).unwrap() > 0);
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n").unwrap();
            socket
                .write_all(&vec![b'a'; MAX_RESPONSE_BYTES as usize + 1])
                .unwrap();
            socket.flush().unwrap();
            close_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            ended_tx.send(()).unwrap();
        });
        assert_eq!(
            Client::new(&endpoint, None, Duration::from_secs(2)).predict(&tokens()),
            Err(Failure::InvalidResponse)
        );
        assert_eq!(ended_rx.try_recv(), Err(mpsc::TryRecvError::Empty));
        close_tx.send(()).unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn aligned_response_uses_only_labels_and_sends_only_tokens_with_backend_auth() {
        for label in ["none", "Lz", "Lt", "Ln", "Lr", "Lp"] {
            let body = format!(
                r#"{{"tokens":["tes","yeux"],"liaison":["{label}","none"],"g2p":["invented","ignored"],"pos":false,"morpho":null}}"#
            );
            let (endpoint, request, worker) = server("200 OK", &body, Duration::ZERO);
            let result = Client::new(&endpoint, Some("test-token".into()), Duration::from_secs(1))
                .predict(&tokens())
                .unwrap();
            assert_eq!(
                result[0],
                serde_json::from_str::<Label>(&format!("\"{label}\"")).unwrap()
            );
            let request = request.recv().unwrap();
            assert!(request.starts_with("POST /g2p/analyser HTTP/1.1"));
            assert!(request
                .to_ascii_lowercase()
                .contains("authorization: bearer test-token"));
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(
                    request.split_once("\r\n\r\n").unwrap().1
                )
                .unwrap(),
                serde_json::json!({"tokens":tokens()})
            );
            worker.join().unwrap();
        }
        assert_eq!(Label::None.phone(), None);
        for (label, phone) in [
            (Label::Lz, "fr/z"),
            (Label::Lt, "fr/t"),
            (Label::Ln, "fr/n"),
            (Label::Lr, "fr/r"),
            (Label::Lp, "fr/p"),
        ] {
            assert_eq!(label.phone(), Some(phone));
        }
    }

    #[test]
    fn malformed_echo_order_lengths_and_unknown_labels_fail_without_payloads() {
        for body in [
            "not JSON: PRIVATE LYRIC",
            r#"{"tokens":["yeux","tes"],"liaison":["Lz","none"]}"#,
            r#"{"tokens":["tes"],"liaison":["Lz","none"]}"#,
            r#"{"tokens":["tes","yeux"],"liaison":["Lz"]}"#,
            r#"{"tokens":["tes","yeux"],"liaison":["Lx","none"]}"#,
            r#"{"tokens":["tes","yeux"]}"#,
            r#"{"tokens":["Tes","yeux"],"liaison":["Lz","none"]}"#,
        ] {
            let (endpoint, request, worker) = server("200 OK", body, Duration::ZERO);
            let error = Client::new(&endpoint, None, Duration::from_secs(1))
                .predict(&tokens())
                .unwrap_err();
            assert_eq!(error, Failure::InvalidResponse);
            assert!(!error.message().contains("PRIVATE"));
            request.recv().unwrap();
            worker.join().unwrap();
        }
    }

    #[test]
    fn http_redirect_network_timeout_and_bounds_are_safe_failures() {
        for status in ["429 Too Many Requests", "503 Unavailable", "302 Found"] {
            let (endpoint, request, worker) = server(status, "PRIVATE LYRIC", Duration::ZERO);
            assert_eq!(
                Client::new(&endpoint, None, Duration::from_secs(1)).predict(&tokens()),
                Err(Failure::Http)
            );
            request.recv().unwrap();
            worker.join().unwrap();
        }
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        assert_eq!(
            Client::new(&endpoint, None, Duration::from_secs(1)).predict(&tokens()),
            Err(Failure::Network)
        );
        let (endpoint, request, worker) = server("200 OK", "{}", Duration::from_millis(150));
        assert_eq!(
            Client::new(&endpoint, None, Duration::from_millis(50)).predict(&tokens()),
            Err(Failure::Timeout)
        );
        request.recv().unwrap();
        worker.join().unwrap();
        let client = Client::new("http://127.0.0.1:1", None, Duration::from_secs(1));
        for input in [
            vec!["a".into(); MAX_TOKENS + 1],
            vec!["a".repeat(MAX_REQUEST_BYTES), "yeux".into()],
        ] {
            assert_eq!(client.predict(&input), Err(Failure::Limit));
        }
        let (endpoint, request, worker) = server(
            "200 OK",
            &"a".repeat(MAX_RESPONSE_BYTES as usize + 1),
            Duration::ZERO,
        );
        assert_eq!(
            Client::new(&endpoint, None, Duration::from_secs(1)).predict(&tokens()),
            Err(Failure::InvalidResponse)
        );
        request.recv().unwrap();
        worker.join().unwrap();
    }
}
