use instructor_ai::decisions::{
    Answers, Choice, Choices, DecisionError, DecisionModel, DecisionRequest, DecisionsConfig,
    Level, NamedQuestion, Noul, PreparedDecision, Question, Score,
};
use mockito::{Matcher, Server};
use serde_json::{json, Value};
use std::time::Duration;

const OPENAI: &str = "openai/gpt-6-luna";
const TYPESAFE: &str = "typesafe/jev-latest";
const OPENROUTER: &str = "openrouter/typesafe/jev-1.13";
const ROUTES: [(&str, &str, &str, &str); 3] = [
    (
        OPENAI,
        "https://api.openai.com",
        "/v1/decisions",
        "OPENAI_API_KEY",
    ),
    (
        TYPESAFE,
        "https://api.typesafe.ai",
        "/v1/systemone",
        "TYPESAFE_API_KEY",
    ),
    (
        OPENROUTER,
        "https://openrouter.ai",
        "/api/alpha/decisions",
        "OPENROUTER_API_KEY",
    ),
];

/// Which team handles {{ item.kind }}?
#[derive(Debug, Choices, PartialEq, serde::Serialize)]
enum Team {
    #[choice(value = "support", description = json!({"rule": "Account access"}), examples = ["Locked out", 3])]
    Help,
    /// Subscription charges
    Billing,
    Other,
}

#[derive(Debug, DecisionModel, PartialEq, serde::Serialize)]
#[decision(validate = check_ticket)]
struct Ticket {
    #[serde(rename = "department")]
    team: Team,
    #[decision(Noul::new(json!({
        "question": "Does the {{ item.kind }} require a refund?",
        "rules": ["{{ item.kind | upper }}", {"fixed": [2, true, null]}]
    }))
        .when_true("Charge dispute")
        .when_false(json!(["Product question"]))
        .example("Duplicate charge", true)
        .example("How do I sign in?", false))]
    refund: f64,
    #[decision(Score::new("How urgent?", [
        Level::new("Routine").example("Where are my invoices?"),
        Level::new("Disputed"),
        Level::new(json!({"meaning": "Blocking"})).example("No access")
    ]).scale(0.0, 10.0).example("Repeated lockout", 2))]
    urgency: f64,
    #[decision(Question::choices("Which channel?", [
        Choice::new("email").description("Email message"),
        Choice::new("chat"),
    ]))]
    channel: String,
}

fn check_ticket(ticket: &mut Ticket, context: &Value, strict: bool) -> Result<(), String> {
    if strict && context["blocked"] == ticket.channel {
        return Err("blocked channel".into());
    }
    Ok(())
}

fn context() -> Value {
    json!({"item": {"kind": "ticket", "body": "Problème de connexion"}, "blocked": "chat"})
}

fn expected_ticket() -> Ticket {
    Ticket {
        team: Team::Help,
        refund: 0.4,
        urgency: 7.0,
        channel: "email".into(),
    }
}

fn jev_request(model: &str) -> Value {
    json!({
        "model": model, "state": context(), "questions": {
            "team": {
                "type": "choice", "instructions": "Which team handles ticket?",
                "criteria": {
                    "support": {"meaning":{"rule":"Account access"}, "examples":["Locked out","3"]},
                    "billing": "Subscription charges", "other": null
                }
            },
            "refund": {
                "type": "noul",
                "instructions": {
                    "question": "Does the ticket require a refund?",
                    "rules": ["TICKET", {"fixed": [2, true, null]}]
                },
                "criteria": {
                    "true": {"meaning":"Charge dispute", "examples":["Duplicate charge"]},
                    "false": {"meaning":["Product question"], "examples":["How do I sign in?"]}
                }
            },
            "urgency": {
                "type": "score", "instructions": "How urgent?", "criteria": [
                    {"meaning":"Routine", "examples":["Where are my invoices?"]},
                    "Disputed",
                    {"meaning":{"meaning":"Blocking"}, "examples":["No access", "Repeated lockout"]}
                ]
            },
            "channel": {
                "type":"choice", "instructions":"Which channel?",
                "criteria":{"email":"Email message", "chat":null}
            }
        }
    })
}

fn jev_response() -> Value {
    json!({
        "answers": {
            "team":{"type": "choice", "choice": "support", "probabilities":{"support":0.9}, "confidence":0.8},
            "refund":{"type": "noul", "noul": 0.4},
            "urgency":{"type": "score", "score": 1.4, "legend":{"1":"Disputed"}},
            "channel":{"type":"choice", "choice":"email"},
            "future_extra_answer":{"type":"opaque"}
        },
        "usage":{"cost":0.003}, "provider_specific":{"version":2}
    })
}

fn openai_response() -> Value {
    json!({
        "answers":[
            {"name":"team", "type":"choice", "choice":"support",
             "probabilities":[{"value":"support", "probability":0.9}]},
            {"name":"refund", "type":"predicate", "probability":0.4},
            {"name":"urgency", "type":"score", "score":1.4, "confidence":0.7},
            {"name":"channel", "type":"choice", "choice":"email"}
        ],
        "usage":{"input_tokens":27}, "provider_specific": [1,2]
    })
}

fn prepared(route: &str) -> PreparedDecision<Ticket> {
    PreparedDecision::new(route, context()).unwrap()
}

fn parse_json_string(value: &Value) -> Value {
    serde_json::from_str(value.as_str().unwrap()).unwrap()
}

fn native_items() -> Value {
    json!([{"role":"user", "content":[
        {"type":"input_json", "data":{"invoice":101}},
        {"type":"input_image", "image_url":"https://example.test/invoice.png"}
    ]}])
}

#[test]
fn jev_schema_and_raw_response_are_provider_independent() {
    for (route, model) in [(OPENROUTER, "typesafe/jev-1.13"), (TYPESAFE, "jev-latest")] {
        let prepared = prepared(route);
        assert_eq!(prepared.body(), &jev_request(model));
        let result: Ticket = prepared.parse(&jev_response()).unwrap();
        assert_eq!(result, expected_ticket());
        let serialized = serde_json::to_value(result).unwrap();
        assert_eq!(serialized["department"], "Help");
        assert!(serialized.get("team").is_none());
    }
}

#[test]
fn openai_translates_and_preserves_order_and_structured_metadata() {
    let prepared = prepared(OPENAI);
    let body = prepared.body();
    let common = &jev_request("model")["questions"];
    assert_eq!(body["model"], "gpt-6-luna");
    assert_eq!(parse_json_string(&body["input"]), context());
    assert!(body.get("state").is_none());
    let questions = body["questions"].as_array().unwrap();
    assert_eq!(questions.len(), 4);
    assert_eq!(questions[0]["type"], "choice");
    assert_eq!(questions[0]["name"], "team");
    assert_eq!(questions[0]["instructions"], common["team"]["instructions"]);
    let choices = &questions[0]["choices"];
    assert_eq!(choices[0]["value"], "support");
    assert_eq!(
        parse_json_string(&choices[0]["description"]),
        common["team"]["criteria"]["support"]
    );
    assert_eq!(
        choices[1],
        json!({"value":"billing", "description":"Subscription charges"})
    );
    assert_eq!(choices[2], json!({"value":"other"}));

    assert_eq!(questions[1]["name"], "refund");
    assert_eq!(questions[1]["type"], "predicate");
    assert!(questions[1].get("criteria").is_none());
    let (instructions, criteria) = questions[1]["instructions"]
        .as_str()
        .unwrap()
        .split_once("\nCriteria: ")
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(instructions).unwrap(),
        common["refund"]["instructions"]
    );
    assert_eq!(
        serde_json::from_str::<Value>(criteria).unwrap(),
        common["refund"]["criteria"]
    );

    assert_eq!(questions[2]["type"], "score");
    assert_eq!(questions[2]["name"], "urgency");
    let levels = &questions[2]["levels"];
    assert_eq!(levels[0]["label"], "0");
    assert_eq!(
        parse_json_string(&levels[0]["description"]),
        common["urgency"]["criteria"][0]
    );
    assert_eq!(levels[1], json!({"label":"1", "description":"Disputed"}));
    assert_eq!(levels[2]["label"], "2");
    assert_eq!(
        parse_json_string(&levels[2]["description"]),
        common["urgency"]["criteria"][2]
    );
    assert_eq!(
        prepared.parse(&openai_response()).unwrap(),
        expected_ticket()
    );
}

#[test]
fn instructions_follow_annotation_field_doc_enum_doc_precedence() {
    #[derive(DecisionModel)]
    struct Example {
        /// Field override
        first: Team,
        /// This loses to explicit instructions
        #[decision(Question::new(json!(["Explicit", "{{ name | default('fallback') }}"])))]
        second: Team,
        #[decision(Noul::new("OK?").criteria(json!({"false":["no"]})).example("yes",true))]
        ok: f64,
    }
    let prepared = PreparedDecision::<Example>::new(OPENROUTER, json!({})).unwrap();
    let q = &prepared.body()["questions"];
    assert_eq!(q["first"]["instructions"], "Field override");
    assert_eq!(q["second"]["instructions"], json!(["Explicit", "fallback"]));
    assert_eq!(
        q["ok"]["criteria"],
        json!({"false":["no"],"true":{"meaning":null,"examples":["yes"]}})
    );
    let answer = prepared
        .parse(&json!({"answers":{
            "first":{"type":"choice","choice":"other"},
            "second":{"type":"choice","choice":"billing"},
            "ok":{"type":"noul","noul":1}
        }}))
        .unwrap();
    assert_eq!(
        (answer.first, answer.second, answer.ok),
        (Team::Other, Team::Billing, 1.0)
    );
}

#[test]
fn native_input_and_validator_context_have_distinct_roles() {
    for input in [json!("Please review"), native_items()] {
        let request = DecisionRequest::input(input.clone()).with_context(context());
        for route in [TYPESAFE, OPENROUTER] {
            assert!(matches!(
                PreparedDecision::<Ticket>::new(route, request.clone()),
                Err(DecisionError::Request(_))
            ));
        }
        let prepared = PreparedDecision::<Ticket>::new(OPENAI, request.clone()).unwrap();
        assert_eq!(prepared.body()["input"], input);
        assert_eq!(
            prepared.body()["questions"][0]["instructions"],
            "Which team handles ticket?"
        );
        let mut raw = openai_response();
        raw["answers"][3]["choice"] = "chat".into();
        assert!(matches!(
            prepared.parse(&raw),
            Err(DecisionError::Validation(_))
        ));
        let lenient = PreparedDecision::<Ticket>::new(OPENAI, request.strict(false)).unwrap();
        assert_eq!(lenient.parse(&raw).unwrap().channel, "chat");
        raw["answers"][3]["choice"] = "unknown".into();
        assert!(matches!(lenient.parse(&raw), Err(DecisionError::Answer(_))));
    }
    for invalid in [json!(true), json!(null), json!({}), json!(["string"])] {
        let request = DecisionRequest::input(invalid).with_context(context());
        assert!(PreparedDecision::<Ticket>::new(OPENAI, request).is_err());
    }
}

#[test]
fn malformed_and_misordered_answers_fail_in_both_dialects() {
    let jev = prepared(OPENROUTER);
    let openai = prepared(OPENAI);
    for raw in [
        json!({}),
        json!([]),
        json!({"answers":null}),
        json!({"answers":{}}),
        json!({"answers":[]}),
    ] {
        assert!(jev.parse(&raw).is_err());
        assert!(openai.parse(&raw).is_err());
    }
    assert!(jev.parse(&openai_response()).is_err());
    assert!(openai.parse(&jev_response()).is_err());
    let mut raw = openai_response();
    raw["answers"].as_array_mut().unwrap().swap(0, 3);
    assert!(openai.parse(&raw).is_err());
    raw = openai_response();
    raw["answers"].as_array_mut().unwrap().push(Value::Null);
    assert!(openai.parse(&raw).is_err());

    for (field, index, key, bad) in [
        ("team", 0, "type", json!("refusal")),
        ("team", 0, "choice", json!("Help")),
        ("team", 0, "choice", json!(123)),
        ("refund", 1, "type", json!("score")),
        ("refund", 1, "noul", json!(true)),
        ("refund", 1, "noul", json!("0.9")),
        ("refund", 1, "noul", json!(1.1)),
        ("refund", 1, "noul", json!(-0.01)),
        ("urgency", 2, "score", json!(false)),
        ("urgency", 2, "score", json!(null)),
        ("urgency", 2, "score", json!(2.01)),
        ("urgency", 2, "score", json!(-1)),
    ] {
        let mut raw = jev_response();
        raw["answers"][field][key] = bad.clone();
        assert!(jev.parse(&raw).unwrap_err().to_string().contains(field));
        let mut raw = openai_response();
        let key = if key == "noul" { "probability" } else { key };
        raw["answers"][index][key] = bad;
        assert!(openai.parse(&raw).unwrap_err().to_string().contains(field));
    }
}

#[test]
fn numeric_boundaries_and_overflow_safe_scaling() {
    #[derive(DecisionModel)]
    struct Rating {
        #[decision(Noul::new("Certain?"))]
        certainty: f64,
        #[decision(Score::new("Rating?", ["low","high"]).scale(-f64::MAX, f64::MAX))]
        scaled: f64,
        #[decision(Score::new("Rating?", ["low","med","high"]))]
        original: f64,
    }
    let prepared = PreparedDecision::<Rating>::new(TYPESAFE, json!({})).unwrap();
    assert!(prepared.body()["questions"]["certainty"]
        .get("criteria")
        .is_none());
    for (raw, scaled) in [(0.0, -f64::MAX), (0.5, 0.0), (1.0, f64::MAX)] {
        let result = prepared
            .parse(&json!({"answers":{
                "certainty":{"type":"noul","noul":raw},
                "scaled":{"type":"score","score":raw},
                "original":{"type":"score","score":raw*2.0},
            }}))
            .unwrap();
        assert_eq!(result.certainty, raw);
        assert_eq!(result.scaled, scaled);
        assert_eq!(result.original, raw * 2.0);
    }
}

#[test]
fn invalid_templates_context_and_provider_routes_are_rejected() {
    for context in [
        json!("text"),
        json!(false),
        json!(null),
        json!({}),
        json!({"item":{}}),
    ] {
        assert!(matches!(
            PreparedDecision::<Ticket>::new(TYPESAFE, context),
            Err(DecisionError::Request(_))
        ));
    }
    for route in [
        "anthropic/model",
        "unknown/model",
        "openai",
        "typesafe/",
        "openrouter/  ",
    ] {
        assert!(matches!(
            DecisionsConfig::new(route),
            Err(DecisionError::Request(_))
        ));
    }
}

#[test]
fn invalid_question_specs_fail_before_transport() {
    // This manual model exercises the same contract as generated models, including
    // configurations assembled dynamically in application code.
    macro_rules! check {
        ($ty:ty, $spec:expr, $expected:expr) => {{
            struct Dynamic;
            impl DecisionModel for Dynamic {
                fn questions() -> Vec<NamedQuestion> {
                    vec![Question::<$ty>::from($spec).named("value")]
                }
                fn from_answers(_: Answers) -> Result<Self, DecisionError> {
                    Ok(Dynamic)
                }
            }
            assert_eq!(
                PreparedDecision::<Dynamic>::new(TYPESAFE, json!({})).is_ok(),
                $expected
            );
        }};
    }
    check!(String, Question::choices("?", [""]), false);
    check!(String, Question::choices("?", ["dup", "dup"]), false);
    check!(
        String,
        Question::choices("?", [Choice::new("one").description(3)]),
        false
    );
    check!(
        String,
        Question::choices("?", (0..255).map(|i| i.to_string())),
        true
    );
    check!(
        String,
        Question::choices("?", (0..256).map(|i| i.to_string())),
        false
    );
    check!(String, Question::choices("?", Vec::<String>::new()), false);
    check!(String, Question::choices("?", ["only"]), true);
    check!(f64, Noul::new(json!({})), false);
    check!(f64, Noul::new(json!([])), false);
    check!(f64, Noul::new(""), false);
    check!(f64, Noul::new(false), false);
    check!(f64, Noul::new("?").criteria(json!({"no":"no"})), false);
    check!(f64, Noul::new("?").criteria(json!([])), false);
    check!(
        f64,
        Noul::new("?").when_true("yes").criteria(json!({})),
        false
    );
    check!(f64, Noul::new("?").when_false(true), false);
    check!(f64, Score::new("?", ["x"; 1]), false);
    check!(f64, Score::new("?", ["x"; 10]).example("hi", 9), true);
    check!(f64, Score::new("?", ["x"; 11]), false);
    check!(f64, Score::new("?", [Value::Null, json!("valid")]), false);
    check!(f64, Score::new("?", ["x"; 2]).example("hi", 2), false);
    check!(f64, Score::new("?", ["x"; 2]).scale(2.0, 1.0), false);
    check!(f64, Score::new("?", ["x"; 2]).scale(2.0, 2.0), false);
    check!(f64, Score::new("?", ["x"; 2]).scale(f64::NAN, 2.0), false);
    check!(
        f64,
        Score::new("?", ["x"; 2]).scale(0.0, f64::INFINITY),
        false
    );
}

fn fixture(route: &str) -> (DecisionRequest, Value, Value) {
    if route == OPENAI {
        let request = DecisionRequest::input(native_items()).with_context(context());
        let body = PreparedDecision::<Ticket>::new(route, request.clone())
            .unwrap()
            .body()
            .clone();
        (request, body, openai_response())
    } else {
        (
            context().into(),
            jev_request(route.split_once('/').unwrap().1),
            jev_response(),
        )
    }
}

#[test]
fn blocking_provider_http_contracts() {
    let mut server = Server::new();
    for (route, base, path, _) in ROUTES {
        let config = || DecisionsConfig::new(route).unwrap().api_key("test-key");
        let client = config().build_blocking().unwrap();
        assert_eq!(client.endpoint(), format!("{base}{path}"));
        assert_eq!(client.model(), route.split_once('/').unwrap().1);
        let (request, body, raw) = fixture(route);
        let mock = server
            .mock("POST", path)
            .match_header("authorization", "Bearer test-key")
            .match_body(Matcher::Json(body))
            .with_body(raw.to_string())
            .expect(2)
            .create();
        let http = reqwest::blocking::Client::builder()
            .no_proxy()
            .build()
            .unwrap();
        let client = config()
            .endpoint(server.url() + path)
            .with_blocking_client(http.clone())
            .unwrap();
        let result: Ticket = client.create(request.clone()).unwrap();
        assert_eq!(result, expected_ticket());
        let (result, completion) = client.create_with_completion::<Ticket>(request).unwrap();
        assert_eq!(result, expected_ticket());
        assert_eq!(completion, raw);
        mock.assert();
        mock.remove();
        let rejected = server
            .mock("POST", path)
            .with_status(503)
            .with_header("x-request-id", "req_unavailable")
            .with_body([0xff])
            .create();
        let err = client.create::<Ticket>(context()).unwrap_err();
        let details = err.http_details().unwrap();
        assert_eq!(details.status, 503);
        assert_eq!(details.request_id.as_deref(), Some("req_unavailable"));
        assert_eq!(details.code, None);
        assert!(matches!(err, DecisionError::Http { body, .. } if body == [0xff]));
        rejected.assert();
    }
}

#[tokio::test]
async fn async_provider_http_contracts() {
    let mut server = Server::new_async().await;
    for (route, _, path, _) in ROUTES {
        let (request, body, raw) = fixture(route);
        let mock = server
            .mock("POST", path)
            .match_header("authorization", "Bearer api-key")
            .match_body(Matcher::Json(body))
            .with_body(raw.to_string())
            .expect(2)
            .create_async()
            .await;
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let client = DecisionsConfig::new(route)
            .unwrap()
            .api_key("api-key")
            .endpoint(server.url() + path)
            .with_client(http)
            .unwrap();
        let result: Ticket = client.create(request.clone()).await.unwrap();
        assert_eq!(result, expected_ticket());
        let (result, completion) = client
            .create_with_completion::<Ticket>(request)
            .await
            .unwrap();
        assert_eq!(result, expected_ticket());
        assert_eq!(completion, raw);
        mock.assert_async().await;
    }
}

#[tokio::test]
async fn openrouter_errors_fail_once_and_client_recovers() {
    let mut server = Server::new_async().await;
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let path = "/api/alpha/decisions";
    let client = DecisionsConfig::new(OPENROUTER)
        .unwrap()
        .api_key("api-key")
        .endpoint(server.url() + path)
        .timeout(Some(Duration::from_secs(5)))
        .with_client(http)
        .unwrap();
    let unexpected = server.mock("POST", path).expect(0).create_async().await;
    let err = client
        .create::<Ticket>(DecisionRequest::input("text"))
        .await
        .unwrap_err();
    assert!(matches!(err, DecisionError::Request(_)));
    unexpected.assert_async().await;
    unexpected.remove_async().await;
    for (status, body) in [
        (429, r#"{"error":{"message":"rate limited","code":429}}"#),
        (200, "{"),
        (200, "{}"),
    ] {
        let mock = server
            .mock("POST", path)
            .with_status(status)
            .with_header("x-request-id", "req_decision")
            .with_body(body)
            .expect(1)
            .create_async()
            .await;
        let error = client.create::<Ticket>(context()).await.unwrap_err();
        if status == 429 {
            let details = error.http_details().unwrap();
            assert_eq!(details.status, 429);
            assert_eq!(details.request_id.as_deref(), Some("req_decision"));
            assert_eq!(details.code.as_deref(), Some("429"));
        }
        match (status, body, error) {
            (429, expected, DecisionError::Http { status, body, .. }) => {
                assert_eq!(status, 429);
                assert_eq!(body, expected.as_bytes());
            }
            (200, "{", DecisionError::Json(_)) | (200, "{}", DecisionError::Answer(_)) => {}
            (_, _, e) => panic!("unexpected error: {e}"),
        }
        mock.assert_async().await;
        mock.remove_async().await;
    }
    let mock = server
        .mock("POST", path)
        .with_body(jev_response().to_string())
        .expect(2)
        .create_async()
        .await;
    let mut invalid = context();
    invalid["blocked"] = "email".into();
    assert!(matches!(
        client.create::<Ticket>(invalid).await,
        Err(DecisionError::Validation(_))
    ));
    assert_eq!(
        client.create::<Ticket>(context()).await.unwrap(),
        expected_ticket()
    );
    mock.assert_async().await;
}

#[test]
fn provider_key_is_scoped_and_explicit_key_wins() {
    // Isolate environment changes from concurrent tests.
    const CHILD: &str = "INSTRUCTOR_DECISIONS_TEST_ROUTE";
    if let Ok(route) = std::env::var(CHILD) {
        let mut server = Server::new();
        for (candidate, _, path, key) in ROUTES {
            if candidate != route {
                assert!(
                    matches!(DecisionsConfig::new(candidate).unwrap().build_blocking(),
                    Err(DecisionError::Request(e)) if e.contains(key))
                );
                continue;
            }
            for expected in ["env-key", "explicit-key"] {
                let (request, _, raw) = fixture(candidate);
                let mock = server
                    .mock("POST", path)
                    .match_header("authorization", format!("Bearer {expected}").as_str())
                    .with_body(raw.to_string())
                    .create();
                let mut config = DecisionsConfig::new(candidate)
                    .unwrap()
                    .endpoint(server.url() + path);
                if expected == "explicit-key" {
                    config = config.api_key(expected);
                }
                let http = reqwest::blocking::Client::builder()
                    .no_proxy()
                    .build()
                    .unwrap();
                config
                    .with_blocking_client(http)
                    .unwrap()
                    .create::<Ticket>(request)
                    .unwrap();
                mock.assert();
            }
        }
    } else {
        for (route, _, _, key) in ROUTES {
            let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
            for (_, _, _, other) in ROUTES {
                cmd.env_remove(other);
            }
            let out = cmd
                .env(CHILD, route)
                .env(key, "env-key")
                .args(["--exact", "provider_key_is_scoped_and_explicit_key_wins"])
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stdout)
            );
        }
    }
}

#[test]
fn shared_model_attributes_apply_to_decisions_and_scaled_values() {
    #[instructor_ai::model]
    #[derive(Choices, Debug)]
    #[description("Where should this ticket go?")]
    #[validate(context = "available_team")]
    enum Department {
        Technical,
        Sales,
    }

    fn available_team(team: &mut Department, context: &Value) -> Result<(), String> {
        if matches!(team, Department::Sales) && context["closed"] == true {
            return Err("sales closed".into());
        }
        Ok(())
    }

    #[instructor_ai::model]
    #[derive(DecisionModel, Debug)]
    #[validate(custom = "check_team")]
    struct Assignment {
        team: Department,
        #[decision(Score::new("Urgency?", ["low","high"]).scale(0.0,10.0))]
        #[validate(context = "min_priority")]
        priority: f64,
    }
    fn min_priority(value: &mut f64, ctx: &Value) -> Result<(), String> {
        *value = value.max(ctx["min"].as_f64().unwrap_or(0.0));
        Ok(())
    }
    fn check_team(assignment: &Assignment) -> Result<(), String> {
        if matches!(assignment.team, Department::Sales) && assignment.priority > 8.0 {
            return Err("escalate".into());
        }
        Ok(())
    }
    let raw = json!({"answers":{
        "team":{"type":"choice","choice":"sales"},
        "priority":{"type":"score","score":0.6}
    }});
    let prepared = |ctx: Value| PreparedDecision::<Assignment>::new(OPENROUTER, ctx).unwrap();
    let base = prepared(json!({}));
    assert_eq!(
        base.body()["questions"]["team"]["instructions"],
        "Where should this ticket go?"
    );
    assert_eq!(base.parse(&raw).unwrap().priority, 6.0);
    assert_eq!(
        prepared(json!({"min":7})).parse(&raw).unwrap().priority,
        7.0
    );
    assert!(prepared(json!({"min":9}))
        .parse(&raw)
        .unwrap_err()
        .to_string()
        .contains("escalate"));
    assert!(prepared(json!({"closed":true}))
        .parse(&raw)
        .unwrap_err()
        .to_string()
        .contains("sales closed"));
}

#[test]
fn existing_blocking_client_shares_model_credentials_and_transport() {
    use instructor_ai::{BlockingClient, Provider, ProviderConfig};
    let mut server = Server::new();
    let config = ProviderConfig::new(Provider::OpenAI, "gpt-6-luna", "shared")
        .with_base_url(format!("{}/gateway/v1/", server.url()));
    let parent = BlockingClient::new(config).unwrap();
    let decisions = parent.decisions().unwrap();
    let path = "/gateway/v1/decisions";
    let mock = server
        .mock("POST", path)
        .match_header("authorization", "Bearer shared")
        .with_body(openai_response().to_string())
        .create();
    assert_eq!(decisions.model(), "gpt-6-luna");
    assert_eq!(decisions.endpoint(), server.url() + path);
    drop(parent);
    let (ticket, raw) = decisions
        .create_with_completion::<Ticket>(context())
        .unwrap();
    assert_eq!(ticket, expected_ticket());
    assert_eq!(raw, openai_response());
    mock.assert();
}

#[tokio::test]
async fn openrouter_chat_client_uses_alpha_route_for_jev() {
    use instructor_ai::{Client, Provider, ProviderConfig};
    let mut server = Server::new_async().await;
    let route = "typesafe/jev-1.13";
    let config = ProviderConfig::new(Provider::OpenRouter, route, "shared")
        .with_base_url(format!("{}/api/v1/", server.url()));
    let parent = Client::new(config).unwrap();
    let mock = server
        .mock("POST", "/api/alpha/decisions")
        .match_header("authorization", "Bearer shared")
        .match_body(Matcher::Json(jev_request(route)))
        .with_body(jev_response().to_string())
        .create_async()
        .await;
    let ticket: Ticket = parent.decisions().unwrap().create(context()).await.unwrap();
    assert_eq!(ticket, expected_ticket());
    mock.assert_async().await;
    let client = Client::new(ProviderConfig::new(Provider::Anthropic, "model", "key")).unwrap();
    assert!(matches!(client.decisions(), Err(DecisionError::Request(_))));
}

#[tokio::test]
async fn timeout_override_is_scoped_to_decision_calls() {
    let mut server = Server::new_async().await;
    let path = "/decisions";
    let mock = server
        .mock("POST", path)
        .with_body_from_request(|_| {
            std::thread::sleep(Duration::from_millis(180));
            jev_response().to_string().into_bytes()
        })
        .expect(2)
        .create_async()
        .await;
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(4))
        .build()
        .unwrap();
    let config = || {
        DecisionsConfig::new(OPENROUTER)
            .unwrap()
            .api_key("key")
            .endpoint(server.url() + path)
    };
    let fast = config()
        .timeout(Some(Duration::from_millis(70)))
        .with_client(http.clone())
        .unwrap();
    let slow = config().with_client(http).unwrap();
    assert!(
        matches!(fast.create::<Ticket>(context()).await, Err(DecisionError::Transport(e)) if e.is_timeout())
    );
    assert_eq!(
        slow.create::<Ticket>(context()).await.unwrap(),
        expected_ticket()
    );
    mock.assert_async().await;
}
