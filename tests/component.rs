use dekopon_decide_provider::Decide;
use dekopon_provider_sdk::provider::Response;
use dekopon_provider_sdk_testkit::{Harness, HarnessError, HttpScript, Native, conformance};
use serde_json::{Value, json};

fn component() -> std::path::PathBuf {
    std::env::var_os("DEKOPON_PROVIDER_COMPONENT")
        .expect("DEKOPON_PROVIDER_COMPONENT must name the freshly built component")
        .into()
}

fn input(backend: &str, model: &str, require: bool) -> Value {
    json!({"backend":backend,"model":model,"evidence":{"ageMs":17,"tool":"check"},"labels":["original","summary"],"requireProbabilities":require})
}

fn response(status: u16, body: Value) -> Response {
    Response {
        status,
        headers: vec![],
        body: body.to_string().into_bytes(),
    }
}

fn answer(model: &str, probabilities: Option<Value>) -> Value {
    let mut choice = json!({"type":"choice","choice":"summary"});
    if let Some(probabilities) = probabilities {
        choice["probabilities"] = probabilities;
    }
    json!({"model":model,"answers":{"decision":choice}})
}

fn fixture(body: Value) -> HttpScript {
    HttpScript::new("localhost", "POST", response(200, body))
}

fn scripted(body: Value) -> dekopon_provider_sdk_testkit::Run<Decide> {
    let run = Harness::<Decide>::get(component()).http(fixture(body));
    let origin = run.origin().expect("scripted origin").to_owned();
    run.settings(json!({"baseUrl":origin}))
}

fn assert_one_request(request: &dekopon_provider_sdk::provider::Request, uri: &str, model: &str) {
    assert_eq!(request.method, "POST");
    assert_eq!(request.uri, uri);
    assert!(
        request
            .headers
            .iter()
            .all(|h| !h.name.eq_ignore_ascii_case("authorization"))
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&request.body).unwrap(),
        json!({
            "model":model,
            "questions":{"decision":{"type":"choice","instructions":"Choose the best matching label for the supplied evidence.","criteria":{"original":"original","summary":"summary"}}},
            "state":{"ageMs":17,"tool":"check"}
        })
    );
}

#[test]
fn built_luna_posts_one_complete_request_and_returns_resolved_model()
-> Result<(), Box<dyn std::error::Error>> {
    conformance::<Decide>(component())?;
    let model = "openai/gpt-6-luna-decisions";
    let run = scripted(answer(model, Some(json!({"original":0.2,"summary":0.8}))));
    let origin = run.origin().expect("scripted origin").to_owned();
    let output = run.call("decide.choose", input("openrouter-luna", model, true))?;
    assert_eq!(output.status, 0, "{}", output.stderr);
    let sent = output.http_request.as_ref().expect("one request");
    assert_eq!(output.http_calls.len(), 1);
    assert_one_request(sent, &format!("{origin}/api/alpha/decisions"), model);
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout)?,
        json!({"choice":"summary","probabilities":{"original":0.2,"summary":0.8},"backend":"openrouter-luna","model":model})
    );
    assert_eq!(output.stdout.iter().filter(|b| **b == b'\n').count(), 1);
    Ok(())
}

#[test]
fn built_jev_posts_one_complete_request_without_guest_credentials()
-> Result<(), Box<dyn std::error::Error>> {
    let model = "typesafe/jev-1.13";
    let run = scripted(answer(model, Some(json!({"original":0.2,"summary":0.8}))));
    let origin = run.origin().expect("scripted origin").to_owned();
    let output = run.call("decide.choose", input("openrouter-jev", model, true))?;
    assert_eq!(output.status, 0, "{}", output.stderr);
    assert_eq!(output.http_calls.len(), 1);
    assert_one_request(
        output.http_request.as_ref().expect("one request"),
        &format!("{origin}/api/alpha/decisions"),
        model,
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout)?,
        json!({"choice":"summary","probabilities":{"original":0.2,"summary":0.8},"backend":"openrouter-jev","model":model})
    );
    Ok(())
}

#[test]
fn built_refusal_and_missing_required_probabilities_never_return_choice()
-> Result<(), Box<dyn std::error::Error>> {
    let model = "openai/gpt-6-luna-decisions";
    for body in [
        json!({"model":model,"answers":{"decision":{"type":"refusal"}}}),
        answer(model, None),
    ] {
        let output = scripted(body).call("decide.choose", input("openrouter-luna", model, true))?;
        assert_eq!(output.status, 0, "{}", output.stderr);
        assert_eq!(output.http_calls.len(), 1);
        let value: Value = serde_json::from_slice(&output.stdout)?;
        assert_eq!(
            value,
            json!({"refused":true,"backend":"openrouter-luna","model":model})
        );
        assert!(value.get("choice").is_none());
    }
    Ok(())
}

#[test]
fn built_malformed_answers_fail_without_a_choice() -> Result<(), Box<dyn std::error::Error>> {
    let model = "openai/gpt-6-luna-decisions";
    for body in [
        json!({"model":model,"answers":{}}),
        json!({"model":model,"answers":{"decision":{"type":"choice","choice":"unknown"}}}),
        answer(model, Some(json!({"original":0.1,"summary":null}))),
        answer(model, Some(json!({"original":0.1,"summary":0.1}))),
    ] {
        let output =
            scripted(body).call("decide.choose", input("openrouter-luna", model, false))?;
        assert_ne!(output.status, 0);
        assert!(output.stdout.is_empty());
        assert_eq!(output.http_calls.len(), 1);
    }
    Ok(())
}

#[test]
fn built_invalid_input_and_unsupported_model_never_use_http()
-> Result<(), Box<dyn std::error::Error>> {
    for input in [
        json!({"backend":"openrouter-luna","model":"openai/gpt-6-luna-decisions","evidence":{},"labels":["same","same"],"requireProbabilities":false}),
        input("openrouter-luna", "unknown-model", false),
    ] {
        let output = Harness::<Decide>::get(component()).call("decide.choose", input)?;
        assert_ne!(output.status, 0);
        assert!(output.http_calls.is_empty());
        assert!(output.stdout.is_empty());
    }
    Ok(())
}

#[test]
fn built_denied_egress_and_throttle_fail_without_a_choice() -> Result<(), Box<dyn std::error::Error>>
{
    let model = "openai/gpt-6-luna-decisions";
    let denied = Harness::<Decide>::get(component())
        .call("decide.choose", input("openrouter-luna", model, false));
    assert!(
        matches!(denied, Err(HarnessError::Invocation(ref failure)) if failure.http_calls.is_empty())
    );
    let run = Harness::<Decide>::get(component()).http(HttpScript::new(
        "localhost",
        "POST",
        response(429, json!({"error":"rate limited"})),
    ));
    let origin = run.origin().expect("scripted origin").to_owned();
    let throttled = run
        .settings(json!({"baseUrl":origin}))
        .call("decide.choose", input("openrouter-luna", model, false))?;
    assert_ne!(throttled.status, 0);
    assert!(throttled.stdout.is_empty());
    assert_eq!(throttled.http_calls.len(), 1);
    Ok(())
}

#[test]
fn native_base_prefix_keeps_uri_and_invocation_at_one_call() {
    let model = "openai/gpt-6-luna-decisions";
    let native = Native::<Decide>::new()
        .settings(json!({"baseUrl":"https://openrouter.ai/proxy"}))
        .http(HttpScript::new(
            "openrouter.ai",
            "POST",
            response(200, answer(model, None)),
        ));
    let output = native.call(
        "decide.choose",
        &input("openrouter-luna", model, false).to_string(),
    );
    assert_eq!(output.status, 0, "{}", output.stderr);
    let requests = native.requests();
    assert_eq!(requests.len(), 1);
    assert_one_request(
        &requests[0],
        "https://openrouter.ai/proxy/api/alpha/decisions",
        model,
    );
}
