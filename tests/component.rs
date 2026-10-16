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
    json!({"backend":backend,"model":model,"evidence":[{"tool":"bash","outcome":"failure","exitStatus":1,"ageMs":17,"bytes":512,"pinned":false}],"labels":["original","summary"],"requireProbabilities":require})
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
    let expected = concat!(
        "{\"model\":\"MODEL\",",
        "\"questions\":{\"decision\":{\"criteria\":{\"original\":\"original\",\"summary\":\"summary\"},",
        "\"instructions\":\"Choose the best matching label for the supplied evidence.\",\"type\":\"choice\"}},",
        "\"state\":{\"records\":[{\"ageMs\":17,\"bytes\":512,\"exitStatus\":1,\"outcome\":\"failure\",\"pinned\":false,\"tool\":\"bash\"}]}}"
    ).replace("MODEL", model);
    assert_eq!(request.body, expected.as_bytes());
}

#[test]
fn built_luna_posts_one_complete_request_and_returns_resolved_model()
-> Result<(), Box<dyn std::error::Error>> {
    conformance::<Decide>(component())?;
    let model = "openai/gpt-6-luna-decisions";
    let run = scripted(answer(model, Some(json!({"original":0.2,"summary":0.8}))));
    let origin = run.origin().expect("scripted origin").to_owned();
    let output = run.call("decide.choose", input("openrouter-luna", model, false))?;
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
fn built_luna_refuses_required_probabilities_before_http() -> Result<(), Box<dyn std::error::Error>>
{
    let output = Harness::<Decide>::get(component()).call(
        "decide.choose",
        input("openrouter-luna", "openai/gpt-6-luna-decisions", true),
    )?;
    assert_eq!(output.status, 0, "{}", output.stderr);
    assert!(output.http_calls.is_empty());
    assert!(output.http_request.is_none());
    let value: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        value,
        json!({"refused":true,"backend":"openrouter-luna","model":"openai/gpt-6-luna-decisions"})
    );
    assert!(value.get("choice").is_none());
    Ok(())
}

#[test]
fn built_missing_required_probabilities_refuse_without_choice()
-> Result<(), Box<dyn std::error::Error>> {
    let model = "typesafe/jev-1.13";
    let output = scripted(answer(model, None))
        .call("decide.choose", input("openrouter-jev", model, true))?;
    assert_eq!(output.status, 0, "{}", output.stderr);
    assert_eq!(output.http_calls.len(), 1);
    let value: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        value,
        json!({"refused":true,"backend":"openrouter-jev","model":model})
    );
    assert!(value.get("choice").is_none());
    Ok(())
}

#[test]
fn built_malformed_answers_fail_without_a_choice() -> Result<(), Box<dyn std::error::Error>> {
    let model = "openai/gpt-6-luna-decisions";
    for body in [
        json!({"model":model,"answers":{}}),
        json!({"model":model,"answers":{"decision":{"type":"refusal"}}}),
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
fn built_null_probabilities_refuse_without_a_choice() -> Result<(), Box<dyn std::error::Error>> {
    let model = "typesafe/jev-1.13";
    let mut body = answer(model, None);
    body["answers"]["decision"]["probabilities"] = Value::Null;
    let output = scripted(body).call("decide.choose", input("openrouter-jev", model, true))?;
    assert_eq!(output.status, 0, "{}", output.stderr);
    assert_eq!(output.http_calls.len(), 1);
    let value: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        value,
        json!({"refused":true,"backend":"openrouter-jev","model":model})
    );
    assert!(value.get("choice").is_none());
    Ok(())
}

#[test]
fn built_free_text_url_and_object_evidence_refuse_before_http()
-> Result<(), Box<dyn std::error::Error>> {
    let model = "openai/gpt-6-luna-decisions";
    let mut note = input("openrouter-luna", model, true);
    note["evidence"][0]["note"] = json!("see https://example.com/?token=sk-synthetic");
    let mut tool = input("openrouter-luna", model, true);
    tool["evidence"][0]["tool"] = json!("https://example.com");
    let mut object = input("openrouter-luna", model, true);
    object["evidence"] = json!({"tool":"bash","outcome":"failure","exitStatus":1,"ageMs":17,"bytes":512,"pinned":false});
    for payload in [note, tool, object] {
        let output = Harness::<Decide>::get(component()).call("decide.choose", payload)?;
        assert_eq!(output.status, 2, "{}", output.stderr);
        assert_eq!(
            output.stderr.trim(),
            "the input does not match the capability's input schema"
        );
        assert!(output.stdout.is_empty());
        assert!(output.http_calls.is_empty());
        assert!(output.http_request.is_none());
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
fn built_endpoint_error_statuses_have_a_generic_diagnostic()
-> Result<(), Box<dyn std::error::Error>> {
    let model = "typesafe/jev-1.13";
    for status in [400, 404, 422] {
        let run = Harness::<Decide>::get(component()).http(HttpScript::new(
            "localhost",
            "POST",
            response(status, json!({"error":"sk-synthetic vendor body"})),
        ));
        let origin = run.origin().expect("scripted origin").to_owned();
        let output = run
            .settings(json!({"baseUrl":origin}))
            .call("decide.choose", input("openrouter-jev", model, false))?;
        assert_ne!(output.status, 0);
        assert_eq!(
            output.stderr,
            "decision endpoint returned an error status\n"
        );
        assert!(output.stdout.is_empty());
        assert_eq!(output.http_calls.len(), 1);
    }
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
