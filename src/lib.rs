use clap::{Parser, Subcommand};
use dekopon_provider_sdk::provider::endpoint::Base;
use dekopon_provider_sdk::provider::{
    Capability, Code, Failure, Http, Proposal, Provider, Settings, Stdout, Usage,
};
use dekopon_provider_sdk::provider::{Header, HttpError, HttpErrorCode, Request, Response};
use dekopon_provider_sdk::{EffectKind, RiskLevel};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fmt;
use std::io::Write;

const OPENROUTER: Base = Base::from_static("https://openrouter.ai");
const LUNA: &str = "openai/gpt-6-luna-decisions";
const JEV: &str = "typesafe/jev-1.13";
const MAX_RECORDS: usize = 256;
const MAX_LABELS: usize = 32;
const MAX_LABEL_BYTES: usize = 128;
const TOLERANCE: f64 = 0.01;

#[derive(Parser)]
#[command(name = "decide", version, subcommand_required = true)]
pub struct Args {
    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand)]
enum Action {
    Choose { request: String },
}

pub struct Decide;

impl Provider for Decide {
    const ID: &'static str = "decide";
    const COMMAND_WORDS: &'static [&'static str] = &["decide"];
    const DESCRIPTION: &'static str = "Choose an advisory label via native OpenRouter Decisions";
    type Args = Args;
    type Capabilities = (Choose,);

    fn propose(args: Args, _stdin_piped: bool) -> Result<Proposal<Self>, Usage> {
        let Action::Choose { request } = args.action;
        let input = serde_json::from_str(&request)
            .map_err(|_| Usage::new("decide choose: expected a JSON request"))?;
        Ok(Proposal::to::<Choose>(input))
    }
}

#[allow(unsafe_code)]
mod export {
    dekopon_provider_sdk::export!(super::Decide);
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecideSettings {
    #[serde(default = "default_base")]
    base_url: Base,
}

fn default_base() -> Base {
    OPENROUTER
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChooseInput {
    backend: String,
    model: String,
    #[schemars(length(min = 1, max = 256))]
    evidence: Vec<Record>,
    labels: Vec<String>,
    require_probabilities: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Record {
    tool: Tool,
    outcome: Outcome,
    exit_status: Option<u8>,
    age_ms: u64,
    bytes: u64,
    pinned: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum Tool {
    Bash,
    #[serde(rename = "none")]
    Absent,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum Outcome {
    Success,
    Failure,
    #[serde(rename = "none")]
    Absent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecideError {
    InvalidInput,
    UnsupportedProfile,
    InvalidResponse,
    Transport,
    Denied,
    Timeout,
    Throttled,
    Budget,
    Unavailable,
}

impl fmt::Display for DecideError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidInput => "invalid decision input",
            Self::UnsupportedProfile => "unsupported decision profile",
            Self::InvalidResponse => "invalid decision response",
            Self::Transport => "decision transport failed",
            Self::Denied => "decision egress denied",
            Self::Timeout => "decision request timed out",
            Self::Throttled => "decision request throttled",
            Self::Budget => "decision invocation budget exhausted",
            Self::Unavailable => "decision service unavailable",
        })
    }
}

impl Failure for DecideError {
    fn code(&self) -> Code {
        match self {
            Self::InvalidInput => Code::INVALID_INPUT,
            Self::UnsupportedProfile => Code::new("unsupported-profile"),
            Self::InvalidResponse => Code::new("invalid-response"),
            Self::Transport => Code::new("http-failed"),
            Self::Denied => Code::new("egress-denied"),
            Self::Timeout => Code::new("timeout"),
            Self::Throttled => Code::new("throttled"),
            Self::Budget => Code::new("budget-exhausted"),
            Self::Unavailable => Code::new("unavailable"),
        }
    }
}

pub struct Choose;

impl Capability for Choose {
    type Provider = Decide;
    const NAME: &'static str = "choose";
    const DESCRIPTION: &'static str =
        "Choose one offered label from bounded metadata; advisory only";
    const EFFECT: EffectKind = EffectKind::ReadOnly;
    const RISK: RiskLevel = RiskLevel::Low;
    type Input = ChooseInput;
    type Needs = (Settings<DecideSettings>, Http);
    type Error = DecideError;

    fn run(
        input: Self::Input,
        (settings, http): Self::Needs,
        out: &mut Stdout,
    ) -> Result<(), Self::Error> {
        let result = choose(input, settings.into_inner().base_url, &mut |request| {
            http.send(request)
        })?;
        serde_json::to_writer(&mut *out, &result).map_err(|_| DecideError::InvalidResponse)?;
        out.write_all(b"\n")
            .map_err(|_| DecideError::InvalidResponse)
    }
}

#[derive(Serialize)]
#[serde(untagged)]
enum Decision {
    Success {
        choice: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        probabilities: Option<std::collections::BTreeMap<String, f64>>,
        backend: String,
        model: String,
    },
    Refusal {
        refused: bool,
        backend: String,
        model: String,
    },
}

fn refusal(input: &ChooseInput) -> Decision {
    Decision::Refusal {
        refused: true,
        backend: input.backend.clone(),
        model: input.model.clone(),
    }
}

fn choose(
    input: ChooseInput,
    base: Base,
    send: &mut dyn FnMut(Request) -> Result<Response, HttpError>,
) -> Result<Decision, DecideError> {
    if input.labels.len() < 2
        || input.labels.len() > MAX_LABELS
        || input.evidence.is_empty()
        || input.evidence.len() > MAX_RECORDS
        || input.labels.iter().any(|label| {
            label.is_empty()
                || label.len() > MAX_LABEL_BYTES
                || !label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        })
        || input.labels.iter().collect::<BTreeSet<_>>().len() != input.labels.len()
    {
        return Err(DecideError::InvalidInput);
    }
    let capable = match (input.backend.as_str(), input.model.as_str()) {
        ("openrouter-luna", LUNA) => true,
        ("openrouter-jev", JEV) => true,
        #[cfg(test)]
        ("test-incapable", "test/incapable") => false,
        _ => return Err(DecideError::UnsupportedProfile),
    };
    if input.require_probabilities && !capable {
        return Ok(refusal(&input));
    }
    let criteria: std::collections::BTreeMap<_, _> = input
        .labels
        .iter()
        .map(|label| (label.clone(), label.clone()))
        .collect();
    let body = serde_json::to_vec(&json!({
        "model": input.model,
        "questions": {"decision": {"type": "choice", "instructions": "Choose the best matching label for the supplied evidence.", "criteria": criteria}},
        "state": {"records": input.evidence},
    })).map_err(|_| DecideError::InvalidInput)?;
    let uri = base
        .join("/api/alpha/decisions")
        .map_err(|_| DecideError::InvalidInput)?;
    let request = Request::new("POST", uri)
        .map_err(|_| DecideError::InvalidInput)?
        .with_header(
            Header::text("content-type", "application/json")
                .map_err(|_| DecideError::InvalidInput)?,
        )
        .with_body(body);
    let response = send(request).map_err(http_error)?;
    match response.status {
        200..=299 => (),
        401 | 403 => return Err(DecideError::Denied),
        429 => return Err(DecideError::Throttled),
        _ => return Err(DecideError::Unavailable),
    }
    let wire: Value =
        serde_json::from_slice(&response.body).map_err(|_| DecideError::InvalidResponse)?;
    let resolved = wire
        .get("model")
        .and_then(Value::as_str)
        .ok_or(DecideError::InvalidResponse)?;
    if resolved.len() > 128
        || (resolved != input.model && !resolved.starts_with(&format!("{}-", input.model)))
    {
        return Err(DecideError::InvalidResponse);
    }
    let answer = wire
        .pointer("/answers/decision")
        .ok_or(DecideError::InvalidResponse)?;
    if answer.get("refused") == Some(&Value::Bool(true))
        || answer.get("type").and_then(Value::as_str) == Some("refusal")
    {
        return Ok(Decision::Refusal {
            refused: true,
            backend: input.backend,
            model: resolved.to_owned(),
        });
    }
    if answer.get("type").and_then(Value::as_str) != Some("choice") {
        return Err(DecideError::InvalidResponse);
    }
    let choice = answer
        .get("choice")
        .and_then(Value::as_str)
        .ok_or(DecideError::InvalidResponse)?;
    if !input.labels.iter().any(|label| label == choice) {
        return Err(DecideError::InvalidResponse);
    }
    let probabilities = match answer.get("probabilities") {
        None => None,
        Some(Value::Object(map)) => {
            if map.len() != input.labels.len() || map.keys().any(|key| !input.labels.contains(key))
            {
                return Err(DecideError::InvalidResponse);
            }
            let values: std::collections::BTreeMap<String, f64> = map
                .iter()
                .map(|(key, value)| {
                    value
                        .as_f64()
                        .filter(|p| p.is_finite() && (0.0..=1.0).contains(p))
                        .map(|p| (key.clone(), p))
                        .ok_or(DecideError::InvalidResponse)
                })
                .collect::<Result<_, _>>()?;
            if (values.values().sum::<f64>() - 1.0).abs() > TOLERANCE {
                return Err(DecideError::InvalidResponse);
            }
            Some(values)
        }
        _ => return Err(DecideError::InvalidResponse),
    };
    if input.require_probabilities && probabilities.is_none() {
        return Ok(Decision::Refusal {
            refused: true,
            backend: input.backend,
            model: resolved.to_owned(),
        });
    }
    Ok(Decision::Success {
        choice: choice.to_owned(),
        probabilities,
        backend: input.backend,
        model: resolved.to_owned(),
    })
}

fn http_error(error: HttpError) -> DecideError {
    match error.code {
        HttpErrorCode::Denied => DecideError::Denied,
        HttpErrorCode::Timeout => DecideError::Timeout,
        HttpErrorCode::HostCallLimit
        | HttpErrorCode::RequestTooLarge
        | HttpErrorCode::ResponseTooLarge => DecideError::Budget,
        _ => DecideError::Transport,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::sync::mpsc;
    use std::time::Duration;

    fn input(backend: &str, model: &str) -> ChooseInput {
        ChooseInput {
            backend: backend.into(),
            model: model.into(),
            evidence: vec![Record {
                tool: Tool::Bash,
                outcome: Outcome::Failure,
                exit_status: Some(1),
                age_ms: 17,
                bytes: 512,
                pinned: false,
            }],
            labels: vec!["original".into(), "summary".into()],
            require_probabilities: true,
        }
    }

    fn error(code: HttpErrorCode) -> HttpError {
        HttpError {
            code,
            message: "synthetic host failure".into(),
        }
    }

    #[test]
    fn native_test_only_incapable_profile_refuses_before_http() {
        let calls = Cell::new(0);
        let decision = choose(
            input("test-incapable", "test/incapable"),
            OPENROUTER,
            &mut |_| {
                calls.set(calls.get() + 1);
                Err(error(HttpErrorCode::Internal))
            },
        )
        .expect("refusal");
        assert!(matches!(decision, Decision::Refusal { refused: true, .. }));
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn native_stalled_http_timeout_consumes_one_request_without_retry() {
        let calls = Cell::new(0);
        let (_sender, receiver) = mpsc::channel::<()>();
        let result = choose(input("openrouter-luna", LUNA), OPENROUTER, &mut |_| {
            calls.set(calls.get() + 1);
            assert!(matches!(
                receiver.recv_timeout(Duration::from_millis(10)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ));
            Err(error(HttpErrorCode::Timeout))
        });
        assert!(matches!(result, Err(DecideError::Timeout)));
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn native_cancellation_interrupts_stalled_http_without_retry() {
        let calls = Cell::new(0);
        std::thread::scope(|scope| {
            let (cancel, receiver) = mpsc::channel::<()>();
            scope.spawn(move || cancel.send(()).expect("cancel signal"));
            let result = choose(input("openrouter-luna", LUNA), OPENROUTER, &mut |_| {
                calls.set(calls.get() + 1);
                receiver
                    .recv_timeout(Duration::from_secs(1))
                    .expect("cancelled while pending");
                Err(error(HttpErrorCode::Internal))
            });
            assert!(matches!(result, Err(DecideError::Transport)));
        });
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn native_invocation_budget_charges_one_request_and_never_retries() {
        let calls = Cell::new(0);
        let mut budget = 1_u32;
        let result = choose(input("openrouter-jev", JEV), OPENROUTER, &mut |_| {
            calls.set(calls.get() + 1);
            assert_eq!(budget, 1);
            budget -= 1;
            Err(error(HttpErrorCode::HostCallLimit))
        });
        assert!(matches!(result, Err(DecideError::Budget)));
        assert_eq!(budget, 0);
        assert_eq!(calls.get(), 1);
    }
}
