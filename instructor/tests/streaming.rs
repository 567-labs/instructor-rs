use futures_util::{stream, StreamExt};
use instructor_ai::{
    model,
    streaming::{
        iterable, iterable_async, iterable_async_with_context, partial, partial_async,
        partial_async_with_context,
    },
    Error, PartialEvent,
};
use serde_json::{json, Value};
use std::cell::Cell;

#[model]
#[derive(Debug, PartialEq)]
struct Name {
    text: String,
}

#[model]
#[derive(Debug)]
#[serde(rename_all = "camelCase")]
struct Group {
    group_name: String,
    members: Option<Vec<Name>>,
    admin: Name,
}

#[model]
#[derive(Debug)]
struct Count {
    #[validate(custom = "positive")]
    #[validate(context = "scale")]
    value: i64,
}

fn positive(n: &i64) -> Result<(), String> {
    if *n > 0 {
        Ok(())
    } else {
        Err("expected a positive number".into())
    }
}
fn scale(n: &mut i64, context: &Value) -> Result<(), String> {
    *n *= context["factor"].as_i64().unwrap_or(1);
    Ok(())
}

#[test]
fn partials_are_typed_recursively_and_keep_original_required_fields() {
    let mut events = partial::<Group>([
        Ok(r#"{"groupName":"Al"#),
        Ok(r#"pha","members":[{"text":"Ada"},{"text":"Lo"#),
        Ok(r#"u"}],"admin":{"text":"Max"}}"#),
    ]);
    let PartialEvent::Partial(first) = events.next().unwrap().unwrap() else {
        panic!()
    };
    let first: GroupPartial = first;
    assert_eq!(first.group_name.as_deref(), Some("Al"));
    assert!(first.admin.is_none());
    assert!(first.members.is_none());

    let PartialEvent::Partial(second) = events.next().unwrap().unwrap() else {
        panic!()
    };
    let names: Vec<NamePartial> = second.members.unwrap();
    assert_eq!(names[0].text.as_deref(), Some("Ada"));
    assert_eq!(names[1].text.as_deref(), Some("Lo"));

    let PartialEvent::Complete(last) = events.next().unwrap().unwrap() else {
        panic!()
    };
    let last: Group = last;
    assert_eq!(last.group_name, "Alpha");
    assert_eq!(last.members.unwrap()[1].text, "Lou");
    assert_eq!(last.admin.text, "Max");
    assert!(events.next().is_none());
}

#[test]
fn partial_omits_unrepresentable_prefix_of_enum() {
    #[model]
    enum Color {
        Blue,
    }
    #[model]
    struct Item {
        id: u64,
        color: Color,
    }

    let mut iter = partial::<Item>([Ok(r#"{"id":123, "color":"Bl"#), Ok("ue\"}")]);
    let PartialEvent::Partial(item) = iter.next().unwrap().unwrap() else {
        panic!()
    };
    assert_eq!(item.id, Some(123));
    assert!(item.color.is_none());
    assert!(matches!(
        iter.next(),
        Some(Ok(PartialEvent::Complete(Item {
            color: Color::Blue,
            ..
        })))
    ));
}

#[test]
fn temporary_prefix_trim_preserves_spaces_in_the_final_value() {
    let mut values = partial::<Name>([Ok(r#"{"text":"hello "#), Ok(r#"world"}"#)]);
    assert!(
        matches!(values.next(), Some(Ok(PartialEvent::Partial(p))) if p.text.as_deref() == Some("hello"))
    );
    assert!(
        matches!(values.next(), Some(Ok(PartialEvent::Complete(v))) if v.text == "hello world")
    );
}

#[test]
fn validation_and_required_field_checks_are_deferred() {
    let mut iter = partial::<Count>([Ok(r#"{"value":-1"#), Ok("}")]);
    let PartialEvent::Partial(item) = iter.next().unwrap().unwrap() else {
        panic!()
    };
    assert_eq!(item.value, Some(-1));
    assert!(matches!(iter.next(), Some(Err(Error::Validation(_)))));
    assert!(iter.next().is_none());
    assert!(matches!(
        partial::<Name>([Ok("{}")]).next(),
        Some(Err(Error::Parse(_)))
    ));
}

#[test]
fn partial_control_repair_and_surrogates_at_every_split_point() {
    let source = "\u{7f}{\"text\":\"\\\u{0}\"\u{95}雪\n\\uD83D\\uDC99\\t\"}\u{9f}";
    for (index, _) in source.char_indices() {
        let events = partial::<Name>([Ok(&source[..index]), Ok(&source[index..])])
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let PartialEvent::Complete(last) = events.last().unwrap() else {
            panic!()
        };
        assert_eq!(last.text, "\"\u{95}雪💙\t");
    }
}

#[test]
fn final_event_requires_clean_eof() {
    let mut events = partial::<Name>([Ok(r#"{"text":"a"#)]);
    assert!(matches!(events.next(), Some(Ok(PartialEvent::Partial(_)))));
    assert!(matches!(events.next(), Some(Err(Error::Incomplete(_)))));
    assert!(events.next().is_none());
    for suffix in [Ok(" trailing"), Err(Error::Provider("failed".into()))] {
        let mut iter = partial::<Name>([Ok(r#"{"text":"full"}"#), suffix]);
        assert!(iter.next().unwrap().is_err());
        assert!(iter.next().is_none());
    }
}

#[test]
fn scalar_and_collection_previews_use_schema_envelopes() {
    let mut text = partial::<String>([Ok(r#"{"content":"Ad"#), Ok("a\"}")]);
    assert!(matches!(text.next(), Some(Ok(PartialEvent::Partial(s))) if s == "Ad"));
    assert!(matches!(text.next(), Some(Ok(PartialEvent::Complete(s))) if s == "Ada"));

    let mut vec = partial::<Vec<Name>>([Ok(r#"{"tasks":[{"text":"Ad"#), Ok("a\"}]}")]);
    assert!(
        matches!(vec.next(), Some(Ok(PartialEvent::Partial(v))) if v[0].text.as_deref() == Some("Ad"))
    );
    assert!(matches!(vec.next(), Some(Ok(PartialEvent::Complete(v))) if v[0].text == "Ada"));
}

#[test]
fn states_are_request_local() {
    let mut a = partial::<Name>([Ok("{\"text\":\"a\\"), Ok("\"b\"}")]);
    let mut b = partial::<Name>([Ok("{\"text\":\"x"), Ok("y\"}")]);
    assert!(matches!(a.next(), Some(Ok(PartialEvent::Partial(_)))));
    assert!(matches!(b.next(), Some(Ok(PartialEvent::Partial(_)))));
    assert!(matches!(a.next(), Some(Ok(PartialEvent::Complete(n))) if n.text == "a\"b"));
    assert!(matches!(b.next(), Some(Ok(PartialEvent::Complete(n))) if n.text == "xy"));
}

#[test]
fn iterable_ignores_wrapper_siblings_and_preserves_nested_values() {
    let payload = concat!(
        r#"{"before":{"tasks":[1]},"\u0074asks" : ["#,
        r#"{"text":"雪 \"} {"},{"text":"\\"}"#,
        r#"],"after":[{"text":"ignore ]"}]} {"text":"end"}"#
    );
    for (index, _) in payload.char_indices() {
        let values: Vec<String> =
            iterable::<Name>([Ok(&payload[..index]), Ok(""), Ok(&payload[index..])])
                .map(|item| item.unwrap().text)
                .collect();
        assert_eq!(values, ["雪 \"} {", "\\", "end"]);
    }
    assert_eq!(iterable::<Name>([Ok(r#"{"tasks":[]}"#)]).count(), 0);
    let nested = iterable::<Value>([Ok(r#"[{"tasks":[{"tasks":[]}]}]"#)])
        .next()
        .unwrap()
        .unwrap();
    assert_eq!(nested, json!({"tasks":[{"tasks":[]}]}));
}

#[test]
fn scalar_token_spanning_fragments_is_one_item() {
    let values = iterable::<Value>([
        Ok("[1"),
        Ok("23,-1."),
        Ok("5e"),
        Ok("2,tru"),
        Ok("e,null,[]]"),
    ])
    .collect::<Result<Vec<_>, _>>()
    .unwrap();
    assert_eq!(
        values,
        [
            json!(123),
            json!(-150.0),
            json!(true),
            Value::Null,
            json!([])
        ]
    );
}

#[test]
fn items_are_yielded_before_truncation_or_validation_error() {
    for suffix in [r#",{"value":-1}]"#, r#",{"v"#, "", ",]"] {
        let json = format!("[{{\"value\":1}}{suffix}");
        let mut values = iterable::<Count>([Ok(json)]);
        assert_eq!(values.next().unwrap().unwrap().value, 1);
        assert!(values.next().unwrap().is_err());
        assert!(values.next().is_none());
    }
    for invalid in [
        "",
        "{",
        "[{}{}]",
        "{]",
        "[1,]",
        "[,]",
        r#"{"tasks":[] "x":1}"#,
    ] {
        assert!(
            iterable::<Value>([Ok(invalid)]).any(|item| item.is_err()),
            "{invalid}"
        );
    }
}

#[test]
fn synchronous_reader_is_lazy_and_releases_source() {
    struct Dropped<'a>(&'a Cell<bool>);
    impl Drop for Dropped<'_> {
        fn drop(&mut self) {
            self.0.set(true)
        }
    }
    for invalid in [false, true] {
        let released = Cell::new(false);
        let guard = Dropped(&released);
        let tail = std::iter::from_fn(move || {
            let _ = &guard;
            panic!("read ahead");
        });
        let first = if invalid {
            r#"[{"value":0}"#
        } else {
            r#"[{"value":1}"#
        };
        let source = std::iter::once(Ok(first)).chain(tail);
        let mut items = iterable::<Count>(source);
        assert_eq!(items.next().unwrap().is_err(), invalid);
        assert_eq!(released.get(), invalid);
        drop(items);
        assert!(released.get());
    }
}

#[tokio::test]
async fn context_applies_only_to_complete_values_and_each_item() {
    let context = json!({"factor": 3});
    let source = stream::iter([Ok(r#"{"value":2"#), Ok("}")]);
    let mut values = partial_async_with_context::<Count>(source, context.clone()).boxed();
    assert!(
        matches!(values.next().await, Some(Ok(PartialEvent::Partial(n))) if n.value == Some(2))
    );
    assert!(matches!(values.next().await, Some(Ok(PartialEvent::Complete(n))) if n.value == 6));
    let source = stream::iter([Ok(r#"[{"value":2},{"value":4}]"#)]);
    let scaled: Vec<_> = iterable_async_with_context::<Count>(source, context)
        .map(|result| result.unwrap().value)
        .collect()
        .await;
    assert_eq!(scaled, [6, 12]);
}

#[tokio::test]
async fn async_drop_and_transport_error_release_source() {
    struct Guard<'a>(&'a Cell<bool>);
    impl Drop for Guard<'_> {
        fn drop(&mut self) {
            self.0.set(true)
        }
    }
    let closed = Cell::new(false);
    let guard = Guard(&closed);
    let source = async_stream::stream! {
        let _guard = guard;
        yield Ok(r#"[{"text":"first"}"#);
        panic!("unexpected second poll");
    };
    let mut items = iterable_async::<Name>(source).boxed_local();
    assert!(items.next().await.unwrap().is_ok());
    drop(items);
    assert!(closed.get());

    closed.set(false);
    let guard = Guard(&closed);
    let source = async_stream::stream! {
        let _guard = guard;
        yield Ok("{");
        yield Err(Error::Provider("gone".into()));
        panic!("unexpected poll after error");
    };
    let mut values = partial_async::<Name>(source).boxed_local();
    assert!(matches!(
        values.next().await,
        Some(Ok(PartialEvent::Partial(_)))
    ));
    assert!(matches!(values.next().await, Some(Err(Error::Provider(_)))));
    assert!(closed.get());
    assert!(values.next().await.is_none());
}

#[test]
fn deeply_nested_candidates_fail_without_recursing_into_model() {
    let nested = "[".repeat(130);
    assert!(matches!(
        iterable::<Value>([Ok(&nested)]).next(),
        Some(Err(Error::Protocol(_)))
    ));
    let source = format!("{{\"x\":{nested}");
    assert!(matches!(
        partial::<Value>([Ok(source)]).next(),
        Some(Err(Error::Protocol(_)))
    ));
}
