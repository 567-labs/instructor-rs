use futures_util::{stream, TryStreamExt};
use instructor_ai::{
    model,
    streaming::{partial, partial_async_with_context},
    Error, PartialEvent,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[model]
struct Count {
    #[validate(context = "scale")]
    n: i64,
}

fn scale(n: &mut i64, ctx: &Value) -> Result<(), String> {
    if *n < 0 {
        return Err("negative count".into());
    }
    *n *= ctx["factor"].as_i64().unwrap_or(2);
    Ok(())
}

#[model]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Entry {
    Counted {
        #[serde(alias = "count.old")]
        child: Count,
    },
    #[serde(alias = "counted_extra")]
    Raw { child: Value },
}

#[model]
struct Batch {
    items: Vec<Entry>,
}

// Includes the handoff from an open child to a closed child inside a still
// open variant, in both key orders. Check every preview as well as the final.
#[tokio::test]
async fn every_split_inside_tagged_collection_uses_original_input_and_context() {
    for item in [
        r#"{"kind":"counted","count.old":{"n":2},"extra":"ok"}"#,
        r#"{"child":{"n":2},"kind":"counted","extra":"ok"}"#,
    ] {
        let text = format!(r#"{{"items":[{item}]}}"#);
        let tag_end = text.find(r#""counted""#).unwrap() + r#""counted""#.len();
        let child_end = text.find("2}").unwrap() + 2;
        let selected = tag_end.max(child_end);
        for split in 0..=text.len() {
            let chunks = stream::iter([Ok(&text[..split]), Ok(&text[split..])]);
            let mut events: Vec<_> =
                partial_async_with_context::<Batch>(chunks, json!({"factor": 3}))
                    .try_collect()
                    .await
                    .unwrap();
            let Some(PartialEvent::Complete(batch)) = events.pop() else {
                panic!("missing result")
            };
            assert!(matches!(batch.items.as_slice(), [Entry::Counted { child }] if child.n == 6));
            if split >= selected && split < text.len() {
                assert_eq!(events.len(), 1, "missing preview at {split}");
            }
            for event in events {
                let PartialEvent::Partial(snapshot) = event else {
                    panic!("duplicate result")
                };
                let first = snapshot.items.as_deref().and_then(|list| list.first());
                let n = match first {
                    Some(EntryPartial::Counted { child: Some(count) }) => count.n,
                    _ => None,
                };
                if split >= selected {
                    assert_eq!(n, Some(6), "split {split}");
                } else {
                    assert!(
                        n.is_none() || n == Some(2),
                        "early normalization at {split}"
                    );
                }
            }
        }
    }
}

#[test]
fn unfinished_tag_matching_another_variant_cannot_trigger_its_validator() {
    let text = r#"{"items":[{"child":{"n":-1},"k\u0069nd" : "counted\u005fextra"}]}"#;
    for split in 0..=text.len() {
        let mut events = partial::<Batch>([Ok(&text[..split]), Ok(&text[split..])])
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let Some(PartialEvent::Complete(batch)) = events.pop() else {
            panic!()
        };
        assert!(matches!(batch.items.as_slice(), [Entry::Raw {child}] if child["n"] == -1));
    }
}

#[test]
fn selected_variant_reports_invalid_child_before_it_closes() {
    let mut events = partial::<Batch>([Ok(r#"{"items":[{"child":{"n":-1},"kind":"counted""#)]);
    assert!(matches!(events.next(), Some(Err(Error::Validation(e)))
        if e.contains("items: 0: child:") && e.contains("negative count")));
    assert!(events.next().is_none());
}

#[model]
#[serde(tag = "kind", content = "payload")]
enum Adjacent {
    #[serde(alias = "old")]
    Item(Count),
}

#[model]
enum External {
    #[serde(rename = "row")]
    Row(Count, Count),
}

#[model]
struct Tagged {
    a: Adjacent,
    b: External,
}

#[test]
fn adjacent_newtype_and_external_tuple_follow_payload_delimiters() {
    let mut events = partial::<Tagged>([
        Ok(r#"{"a":{"payload":{"n":2},"kind":"old""#),
        Ok(r#"},"b":{"row":[{"n":3},{"n":4"#),
        Ok("}]}}"),
    ]);
    let PartialEvent::Partial(first) = events.next().unwrap().unwrap() else {
        panic!()
    };
    assert!(matches!(first.a, Some(AdjacentPartial::Item(Some(c))) if c.n == Some(4)));
    let PartialEvent::Partial(next) = events.next().unwrap().unwrap() else {
        panic!()
    };
    assert!(
        matches!(next.b, Some(ExternalPartial::Row(Some(a),Some(b))) if a.n==Some(6) && b.n==Some(4))
    );
    let PartialEvent::Complete(last) = events.next().unwrap().unwrap() else {
        panic!()
    };
    assert!(matches!(last.b,External::Row(a,b) if a.n==6 && b.n==8));
}

#[model]
struct Summary {
    #[serde(alias = "stats[0]")]
    counts: Vec<Count>,
    required: String,
}

#[model]
struct Report {
    #[serde(flatten)]
    summary: Summary,
}

#[test]
fn flattened_struct_checks_each_child_without_requiring_parent_fields() {
    let text = r#"{"stats[0]":[{"n":2}],"required":"done"}"#;
    let child_end = text.find('}').unwrap() + 1;
    for split in 1..text.len() {
        let mut events = partial::<Report>([Ok(&text[..split]), Ok(&text[split..])]);
        let PartialEvent::Partial(snapshot) = events.next().unwrap().unwrap() else {
            panic!()
        };
        let n = snapshot
            .summary
            .and_then(|s| s.counts)
            .and_then(|v| v.first().and_then(|c| c.n));
        if split >= child_end {
            assert_eq!(n, Some(4))
        } else {
            assert!(n.is_none() || n == Some(2))
        }
        let PartialEvent::Complete(full) = events.next().unwrap().unwrap() else {
            panic!()
        };
        assert_eq!(full.summary.counts[0].n, 4);
    }
    assert!(
        matches!(partial::<Report>([Ok(r#"{"stats[0]":[{"n":-1}"#)]).next(), Some(Err(Error::Validation(e))) if e.contains("negative count"))
    );
}

#[model]
struct Directory {
    #[serde(flatten)]
    counts: BTreeMap<String, Count>,
    #[serde(alias = "legacy")]
    raw: Value,
}

#[test]
fn flattened_map_excludes_sibling_aliases() {
    let mut events = partial::<Directory>([Ok(r#"{"legacy":{"n":-1},"key":{"n":3}"#), Ok("}")]);
    let PartialEvent::Partial(snapshot) = events.next().unwrap().unwrap() else {
        panic!()
    };
    assert_eq!(snapshot.raw.unwrap()["n"], -1);
    assert_eq!(snapshot.counts.unwrap()["key"].n, Some(6));
    assert!(matches!(events.next(), Some(Ok(PartialEvent::Complete(_)))));
}

#[model]
#[serde(untagged)]
enum Choice {
    Typed { required: String, child: Count },
    Fallback { child: Value },
}

#[model]
struct Ambiguous {
    choice: Choice,
}

#[test]
fn untagged_choice_uses_full_input_to_choose_an_alternative() {
    let mut events = partial::<Ambiguous>([Ok(r#"{"choice":{"child":{"n":-1}"#), Ok("}}")]);
    assert!(matches!(events.next(), Some(Ok(PartialEvent::Partial(_)))));
    let PartialEvent::Complete(full) = events.next().unwrap().unwrap() else {
        panic!()
    };
    assert!(matches!(full.choice, Choice::Fallback {child} if child["n"] == -1));
}
