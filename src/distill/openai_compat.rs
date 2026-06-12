//! A [`Distiller`] backed by an OpenAI-compatible chat completions endpoint
//! (e.g. Ollama or llama-server), behind the `distill-http` feature.
//!
//! This is the only module in the crate that performs HTTP, and only when
//! `distill-http` is enabled. It uses [`reqwest::blocking`] so the crate
//! remains synchronous — async callers should wrap [`Distiller::distill`]
//! calls in `spawn_blocking`.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::distill::{DistilledMemory, Distiller};
use crate::error::Error;
use crate::traits::Result;

/// Default request timeout in seconds.
///
/// 300 seconds (5 minutes) covers an Ollama cold model load.
pub const DEFAULT_TIMEOUT_SECS: u64 = 300;

/// Default maximum transcript length in characters before truncation.
pub const DEFAULT_MAX_TRANSCRIPT_CHARS: usize = 100_000;

/// System prompt sent to the model on every distillation request.
const SYSTEM_PROMPT: &str = "Extract durable memory from this conversation transcript. \
Produce: (1) a summary under 150 words; (2) facts worth remembering across future \
sessions — decisions made and why, corrections the user gave, user preferences, and \
state changes (X is now Y). Each fact must be one self-contained sentence \
understandable without the transcript. Omit pleasantries, transient debugging \
detail, and anything already obvious from a codebase.";

/// Selects the `response_format` payload shape used to request structured
/// JSON output from the chat completions endpoint.
///
/// Ollama's OpenAI-compatible endpoint and llama-server accept different
/// shapes for requesting schema-constrained output; this selects which one
/// is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum SchemaStyle {
    /// `{"type":"json_schema","json_schema":{"name":...,"schema":{...}}}` —
    /// the shape accepted by Ollama's OpenAI-compatible endpoint.
    #[default]
    OpenAi,
    /// `{"type":"json_object","schema":{...}}` — the shape reliably
    /// accepted by llama-server.
    LlamaServer,
}

/// Returns the JSON schema describing [`DistilledMemory`], used in the
/// `response_format` payload and embedded in the fallback prompt.
fn distilled_memory_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "summary": { "type": "string" },
            "facts": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "kind": {
                            "type": "string",
                            "enum": ["decision", "correction", "preference", "state"]
                        },
                        "text": { "type": "string" }
                    },
                    "required": ["kind", "text"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["summary", "facts"],
        "additionalProperties": false
    })
}

/// Builds the `response_format` payload for the given [`SchemaStyle`].
///
/// This is a pure function, unit-testable without any HTTP involved.
#[must_use]
pub fn response_format_payload(style: SchemaStyle) -> Value {
    let schema = distilled_memory_schema();
    match style {
        SchemaStyle::OpenAi => json!({
            "type": "json_schema",
            "json_schema": {
                "name": "distilled_memory",
                "schema": schema
            }
        }),
        SchemaStyle::LlamaServer => json!({
            "type": "json_object",
            "schema": schema
        }),
    }
}

/// Truncates `text` to at most `max_chars` characters, keeping the *end* of
/// the string and cutting only on `char` boundaries.
///
/// If `text` already has at most `max_chars` characters, it is returned
/// unchanged.
#[must_use]
pub fn truncate_keep_end(text: &str, max_chars: usize) -> &str {
    let char_count = text.chars().count();
    if char_count <= max_chars {
        return text;
    }
    let skip = char_count - max_chars;
    match text.char_indices().nth(skip) {
        Some((byte_idx, _)) => &text[byte_idx..],
        None => "",
    }
}

/// Strips a leading/trailing Markdown code fence (```` ``` ```` or ` ```json `)
/// from `content`, if present.
fn strip_code_fences(content: &str) -> &str {
    let trimmed = content.trim();
    let Some(after_open) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    // Skip an optional language tag (e.g. "json") up to the first newline.
    let after_lang = match after_open.find('\n') {
        Some(idx) => &after_open[idx + 1..],
        None => after_open,
    };
    match after_lang.rfind("```") {
        Some(idx) => after_lang[..idx].trim(),
        None => after_lang.trim(),
    }
}

/// `OpenAI` chat completion request body.
#[derive(Debug, Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<Value>,
}

/// A single chat message.
#[derive(Debug, Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: String,
}

/// Minimal subset of an `OpenAI` chat completion response envelope.
#[derive(Debug, Deserialize)]
struct ChatResponse {
    #[serde(default)]
    choices: Vec<ChatChoice>,
}

/// A single choice in a chat completion response.
#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatResponseMessage,
}

/// The message portion of a chat completion choice.
#[derive(Debug, Deserialize)]
struct ChatResponseMessage {
    #[serde(default)]
    content: Option<String>,
}

/// A [`Distiller`] that talks to an OpenAI-compatible `/chat/completions`
/// endpoint such as Ollama or llama-server.
///
/// # HTTP only
///
/// This client is built without a TLS stack and supports `http://` base URLs
/// only — it targets local or LAN/VPN inference endpoints (Ollama,
/// llama-server). An `https://` base URL fails at request time with
/// [`Error::Distill`]. If you need a remote TLS endpoint, implement
/// [`Distiller`] with your own HTTP client.
///
/// # Fallback behaviour
///
/// Some local model servers respond `HTTP 200` with unconstrained text when
/// a requested `response_format` schema is unsupported or silently dropped
/// ("fail open"). To handle this, [`Distiller::distill`] makes at most two
/// HTTP calls:
///
/// 1. With `response_format` set per [`SchemaStyle`]. If the response is a
///    2xx and its content parses as [`DistilledMemory`], this result is
///    returned.
/// 2. Otherwise (non-2xx, or 2xx with unparsable/empty content), retry once
///    with no `response_format` and the JSON schema embedded in the prompt.
///    Markdown code fences are stripped from the response before parsing.
///
/// If the second attempt also fails, [`Error::Distill`] is returned.
#[derive(Debug, Clone)]
pub struct OpenAiCompatDistiller {
    base_url: String,
    model: String,
    schema_style: SchemaStyle,
    max_transcript_chars: usize,
    client: reqwest::blocking::Client,
}

impl OpenAiCompatDistiller {
    /// Creates a new distiller targeting `base_url` (e.g.
    /// `http://127.0.0.1:11434/v1`) with the given `model`.
    ///
    /// Uses [`DEFAULT_TIMEOUT_SECS`] and [`DEFAULT_MAX_TRANSCRIPT_CHARS`];
    /// override these with [`with_timeout_secs`](Self::with_timeout_secs)
    /// and
    /// [`with_max_transcript_chars`](Self::with_max_transcript_chars).
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying HTTP client cannot be
    /// constructed.
    pub fn new(base_url: impl Into<String>, model: impl Into<String>) -> Result<Self> {
        let client = Self::build_client(Duration::from_secs(DEFAULT_TIMEOUT_SECS))?;
        Ok(Self {
            base_url: base_url.into(),
            model: model.into(),
            schema_style: SchemaStyle::default(),
            max_transcript_chars: DEFAULT_MAX_TRANSCRIPT_CHARS,
            client,
        })
    }

    /// Sets the [`SchemaStyle`] used to request structured output.
    #[must_use]
    pub fn with_schema_style(mut self, style: SchemaStyle) -> Self {
        self.schema_style = style;
        self
    }

    /// Sets the request timeout in seconds, rebuilding the underlying HTTP
    /// client.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying HTTP client cannot be
    /// constructed.
    pub fn with_timeout_secs(mut self, timeout_secs: u64) -> Result<Self> {
        self.client = Self::build_client(Duration::from_secs(timeout_secs))?;
        Ok(self)
    }

    /// Sets the maximum number of transcript characters sent to the model;
    /// longer transcripts are truncated from the front, keeping the end.
    #[must_use]
    pub fn with_max_transcript_chars(mut self, max_transcript_chars: usize) -> Self {
        self.max_transcript_chars = max_transcript_chars;
        self
    }

    /// Builds a blocking HTTP client with the given timeout.
    ///
    /// Redirects are disabled: there is no legitimate redirect for a
    /// `/chat/completions` endpoint, and following one would be an
    /// egress-redirection vector.
    fn build_client(timeout: Duration) -> Result<reqwest::blocking::Client> {
        reqwest::blocking::Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| Error::Distill(format!("failed to build HTTP client: {e}")))
    }

    /// Returns the `/chat/completions` URL for this distiller.
    fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
    }

    /// Sends a chat completion request over the wire.
    ///
    /// Returns the raw [`reqwest::Error`] on failure so the caller can
    /// distinguish a transport-level failure (connection refused, timeout,
    /// request build error) from a response that was received but rejected
    /// — only the latter triggers the prompt-embedded fallback.
    fn send_raw(
        &self,
        body: &ChatRequest<'_>,
    ) -> std::result::Result<reqwest::blocking::Response, reqwest::Error> {
        self.client.post(self.endpoint()).json(body).send()
    }

    /// Sends a chat completion request and returns the parsed response
    /// envelope, or an [`Error::Distill`] describing why the HTTP call
    /// itself failed (non-2xx status or non-JSON body).
    ///
    /// Transport-level failures are not represented by this function's
    /// return type — callers should call [`Self::send_raw`] directly when
    /// they need to distinguish transport errors from response rejections.
    fn parse_response(response: reqwest::blocking::Response) -> Result<ChatResponse> {
        if !response.status().is_success() {
            return Err(Error::Distill(format!(
                "non-success status: {}",
                response.status()
            )));
        }

        response
            .json::<ChatResponse>()
            .map_err(|e| Error::Distill(format!("invalid response envelope: {e}")))
    }

    /// Extracts the message content from a chat response, treating a
    /// missing or empty `content` field as an error.
    fn message_content(response: ChatResponse) -> Result<String> {
        let content = response
            .choices
            .into_iter()
            .next()
            .and_then(|choice| choice.message.content);

        match content {
            Some(c) if !c.is_empty() => Ok(c),
            _ => Err(Error::Distill("response had no message content".into())),
        }
    }

    /// Attempt 1: request with `response_format` set, parsing the content
    /// strictly as JSON.
    ///
    /// Returns [`AttemptError::Transport`] if the request itself could not
    /// be sent (connection refused, timed out, or could not be built) — the
    /// caller should not retry via [`Self::attempt_prompt_embedded`] in that
    /// case, since attempt 2 would fail the same way. Any other failure
    /// (non-2xx status, unparsable body) is [`AttemptError::Rejected`].
    fn attempt_structured(
        &self,
        transcript: &str,
    ) -> std::result::Result<DistilledMemory, AttemptError> {
        let body = ChatRequest {
            model: &self.model,
            messages: vec![
                ChatMessage {
                    role: "system",
                    content: SYSTEM_PROMPT.to_owned(),
                },
                ChatMessage {
                    role: "user",
                    content: transcript.to_owned(),
                },
            ],
            response_format: Some(response_format_payload(self.schema_style)),
        };

        let raw = self.send_raw(&body).map_err(AttemptError::from)?;
        let response = Self::parse_response(raw)?;
        let content = Self::message_content(response)?;
        serde_json::from_str(&content).map_err(|_| AttemptError::Rejected)
    }

    /// Attempt 2: request with no `response_format` and the schema embedded
    /// in the prompt; strips Markdown code fences before parsing.
    fn attempt_prompt_embedded(&self, transcript: &str) -> Result<DistilledMemory> {
        let schema = distilled_memory_schema();
        let prompt = format!(
            "{transcript}\n\n---\nRespond with ONLY a JSON object matching this schema, \
and nothing else:\n{schema}"
        );

        let body = ChatRequest {
            model: &self.model,
            messages: vec![
                ChatMessage {
                    role: "system",
                    content: SYSTEM_PROMPT.to_owned(),
                },
                ChatMessage {
                    role: "user",
                    content: prompt,
                },
            ],
            response_format: None,
        };

        let raw = self
            .send_raw(&body)
            .map_err(|e| Error::Distill(format!("request failed: {e}")))?;
        let response = Self::parse_response(raw)?;
        let content = Self::message_content(response)?;
        let stripped = strip_code_fences(&content);
        serde_json::from_str(stripped)
            .map_err(|e| Error::Distill(format!("failed to parse fallback response: {e}")))
    }
}

/// The outcome of [`OpenAiCompatDistiller::attempt_structured`]'s failure
/// modes, distinguishing transport failures (which should not trigger the
/// prompt-embedded fallback) from response rejections (which should).
enum AttemptError {
    /// The request could not be sent at all: connection refused, DNS
    /// failure, timed out, or the request could not be built.
    Transport(reqwest::Error),
    /// A response was received but rejected: non-2xx status, an unparsable
    /// envelope, missing content, or content that did not match
    /// [`DistilledMemory`].
    Rejected,
}

impl From<reqwest::Error> for AttemptError {
    fn from(e: reqwest::Error) -> Self {
        if e.is_timeout() || e.is_connect() || e.is_request() {
            AttemptError::Transport(e)
        } else {
            AttemptError::Rejected
        }
    }
}

impl From<Error> for AttemptError {
    fn from(_: Error) -> Self {
        AttemptError::Rejected
    }
}

impl Distiller for OpenAiCompatDistiller {
    /// # Security
    ///
    /// This implementation transmits `transcript` verbatim to the
    /// configured `base_url` — no secret scrubbing is applied at this
    /// layer. [`ContextForge::distill_and_save`](crate::ContextForge::distill_and_save)
    /// is the only entry point that scrubs secrets (via
    /// [`scrub_secrets`](crate::scrub_secrets)) before a transcript reaches
    /// a [`Distiller`]. Callers invoking [`Distiller::distill`] directly are
    /// responsible for scrubbing first.
    fn distill(&self, transcript: &str) -> Result<DistilledMemory> {
        let truncated = truncate_keep_end(transcript, self.max_transcript_chars);

        match self.attempt_structured(truncated) {
            Ok(memory) => Ok(memory),
            Err(AttemptError::Transport(e)) => Err(Error::Distill(format!("request failed: {e}"))),
            Err(AttemptError::Rejected) => self.attempt_prompt_embedded(truncated),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::distill::FactKind;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    /// A canned HTTP response for the hand-rolled mock server.
    struct MockResponse {
        status_line: &'static str,
        body: String,
    }

    /// Starts a single-connection mock HTTP server on `127.0.0.1:0`,
    /// returning its address and a channel that yields the captured request
    /// body once a connection is handled.
    ///
    /// `responses` is consumed in order across successive connections.
    fn spawn_mock_server(responses: Vec<MockResponse>) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock listener");
        let addr = listener.local_addr().expect("local addr");
        let (tx, rx) = mpsc::channel();

        thread::spawn(move || {
            for response in responses {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };

                let body = read_request_body(&mut stream);
                let _ = tx.send(body);

                let payload = format!(
                    "{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response.status_line,
                    response.body.len(),
                    response.body
                );
                let _ = stream.write_all(payload.as_bytes());
                let _ = stream.flush();
            }
        });

        (format!("http://{addr}"), rx)
    }

    /// Reads headers and the request body (using `Content-Length`) from a
    /// raw HTTP/1.1 request.
    fn read_request_body(stream: &mut std::net::TcpStream) -> String {
        let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
        let mut content_length: usize = 0;

        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            let trimmed = line.trim_end();
            if trimmed.is_empty() {
                break;
            }
            if let Some(value) = trimmed.to_ascii_lowercase().strip_prefix("content-length:") {
                content_length = value.trim().parse().unwrap_or(0);
            }
        }

        let mut body = vec![0u8; content_length];
        if content_length > 0 {
            let _ = reader.read_exact(&mut body);
        }
        String::from_utf8(body).unwrap_or_default()
    }

    fn distiller_for(base_url: &str) -> OpenAiCompatDistiller {
        OpenAiCompatDistiller::new(base_url, "test-model")
            .expect("construct distiller")
            .with_timeout_secs(5)
            .expect("set timeout")
    }

    #[test]
    fn payload_shapes_for_both_styles() {
        let openai = response_format_payload(SchemaStyle::OpenAi);
        assert_eq!(openai["type"], "json_schema");
        assert_eq!(openai["json_schema"]["name"], "distilled_memory");
        assert_eq!(openai["json_schema"]["schema"]["type"], "object");
        assert_eq!(
            openai["json_schema"]["schema"]["required"],
            json!(["summary", "facts"])
        );

        let llama = response_format_payload(SchemaStyle::LlamaServer);
        assert_eq!(llama["type"], "json_object");
        assert_eq!(llama["schema"]["type"], "object");
        assert_eq!(llama["schema"]["required"], json!(["summary", "facts"]));
    }

    #[test]
    fn schema_style_default_is_openai() {
        assert_eq!(SchemaStyle::default(), SchemaStyle::OpenAi);
    }

    #[test]
    fn truncate_keeps_end_and_respects_char_boundaries() {
        // Multibyte characters: each "é" is 2 bytes in UTF-8.
        let text = "ééééé hello world";
        let truncated = truncate_keep_end(text, 11);
        assert_eq!(truncated.chars().count(), 11);
        assert!(truncated.ends_with("hello world"));
        // Must not panic and must be valid UTF-8 (guaranteed by &str type).
        assert!(truncated.is_char_boundary(0));
    }

    #[test]
    fn truncate_noop_when_short_enough() {
        let text = "short";
        assert_eq!(truncate_keep_end(text, 100), text);
        assert_eq!(truncate_keep_end(text, 5), text);
    }

    #[test]
    fn strip_fences_removes_json_fence() {
        let fenced = "```json\n{\"summary\":\"x\",\"facts\":[]}\n```";
        assert_eq!(
            strip_code_fences(fenced),
            "{\"summary\":\"x\",\"facts\":[]}"
        );
    }

    #[test]
    fn strip_fences_removes_plain_fence() {
        let fenced = "```\n{\"summary\":\"x\",\"facts\":[]}\n```";
        assert_eq!(
            strip_code_fences(fenced),
            "{\"summary\":\"x\",\"facts\":[]}"
        );
    }

    #[test]
    fn strip_fences_passthrough_when_unfenced() {
        let plain = "{\"summary\":\"x\",\"facts\":[]}";
        assert_eq!(strip_code_fences(plain), plain);
    }

    #[test]
    fn attempt_one_success_with_valid_envelope() {
        let body = json!({
            "summary": "Discussed deploy fix.",
            "facts": [
                {"kind": "decision", "text": "We decided to roll back the deploy."}
            ]
        })
        .to_string();
        let envelope = json!({
            "choices": [
                {"message": {"content": body}}
            ]
        })
        .to_string();

        let (url, rx) = spawn_mock_server(vec![MockResponse {
            status_line: "HTTP/1.1 200 OK",
            body: envelope,
        }]);

        let distiller = distiller_for(&url);
        let result = distiller.distill("hello transcript").expect("distill ok");

        assert_eq!(result.summary, "Discussed deploy fix.");
        assert_eq!(result.facts.len(), 1);
        assert_eq!(result.facts[0].kind, FactKind::Decision);

        let request_body = rx.recv().expect("captured request");
        assert!(request_body.contains("response_format"));
    }

    #[test]
    fn non_2xx_falls_back_to_prompt_embedded() {
        let fallback_body = json!({
            "summary": "Fallback summary.",
            "facts": []
        })
        .to_string();
        let fallback_envelope = json!({
            "choices": [
                {"message": {"content": fallback_body}}
            ]
        })
        .to_string();

        let (url, rx) = spawn_mock_server(vec![
            MockResponse {
                status_line: "HTTP/1.1 500 Internal Server Error",
                body: "{}".to_owned(),
            },
            MockResponse {
                status_line: "HTTP/1.1 200 OK",
                body: fallback_envelope,
            },
        ]);

        let distiller = distiller_for(&url);
        let result = distiller.distill("hello transcript").expect("distill ok");
        assert_eq!(result.summary, "Fallback summary.");

        // First request used response_format.
        let first = rx.recv().expect("first request");
        assert!(first.contains("response_format"));

        // Second request has NO response_format, and embeds the schema.
        let second = rx.recv().expect("second request");
        assert!(!second.contains("response_format"));
        assert!(second.contains("additionalProperties"));
    }

    #[test]
    fn fails_open_garbage_content_falls_back() {
        let attempt1_envelope = json!({
            "choices": [
                {"message": {"content": "Sure! Here's some unstructured text about the chat."}}
            ]
        })
        .to_string();

        let fallback_body = json!({
            "summary": "Recovered via fallback.",
            "facts": []
        })
        .to_string();
        let fallback_envelope = json!({
            "choices": [
                {"message": {"content": format!("```json\n{fallback_body}\n```")}}
            ]
        })
        .to_string();

        let (url, rx) = spawn_mock_server(vec![
            MockResponse {
                status_line: "HTTP/1.1 200 OK",
                body: attempt1_envelope,
            },
            MockResponse {
                status_line: "HTTP/1.1 200 OK",
                body: fallback_envelope,
            },
        ]);

        let distiller = distiller_for(&url);
        let result = distiller.distill("hello transcript").expect("distill ok");
        assert_eq!(result.summary, "Recovered via fallback.");

        let first = rx.recv().expect("first request");
        assert!(first.contains("response_format"));
        let second = rx.recv().expect("second request");
        assert!(!second.contains("response_format"));
    }

    #[test]
    fn both_attempts_garbage_returns_distill_error() {
        let garbage_envelope = json!({
            "choices": [
                {"message": {"content": "not json at all"}}
            ]
        })
        .to_string();

        let (url, _rx) = spawn_mock_server(vec![
            MockResponse {
                status_line: "HTTP/1.1 200 OK",
                body: garbage_envelope.clone(),
            },
            MockResponse {
                status_line: "HTTP/1.1 200 OK",
                body: garbage_envelope,
            },
        ]);

        let distiller = distiller_for(&url);
        let err = distiller.distill("hello transcript").unwrap_err();
        assert!(matches!(err, Error::Distill(_)));
    }

    #[test]
    fn transport_error_does_not_trigger_fallback() {
        // A listener that accepts a connection and then writes nothing,
        // forcing the client to time out. Records how many connections it
        // accepted so the test can assert there was no second (fallback)
        // attempt.
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock listener");
        let addr = listener.local_addr().expect("local addr");
        let (tx, rx) = mpsc::channel::<()>();

        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else {
                    return;
                };
                let _ = tx.send(());
                // Hold the connection open without responding, then drop it
                // once the test is done so the thread can exit.
                thread::sleep(Duration::from_secs(10));
                drop(stream);
            }
        });

        let url = format!("http://{addr}");
        let distiller = OpenAiCompatDistiller::new(&url, "test-model")
            .expect("construct distiller")
            .with_timeout_secs(1)
            .expect("set timeout");

        let err = distiller.distill("hello transcript").unwrap_err();
        assert!(matches!(err, Error::Distill(_)));

        // Exactly one connection should have been accepted: a transport
        // error (timeout) must not trigger the prompt-embedded fallback.
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok());
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
    }
}
