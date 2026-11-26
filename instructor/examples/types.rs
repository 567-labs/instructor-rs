use instructor_ai::{model, parse_response, response_schema, Error};
use serde_json::Value;

#[model]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Venue {
    OnSite { room: String },
    Remote { url: String },
}

#[model]
struct Meeting {
    /// Title as it appears on the agenda.
    #[serde(rename = "meetingTitle")]
    title: String,
    venue: Venue,
    attendees: Vec<String>,
    duration_minutes: Option<u16>,
}

fn main() -> Result<(), Error> {
    println!("{:#}", response_schema::<Meeting>());
    let json = r#"{
        "meetingTitle": "Design review",
        "venue": {"kind": "on_site", "room": "A"},
        "attendees": ["Ali", "Chen"]
    }"#;
    let meeting: Meeting = parse_response(json, &Value::Null)?;
    let location = match meeting.venue {
        Venue::OnSite { room } => room,
        Venue::Remote { url } => url,
    };
    println!(
        "{} in {location} with {:?}",
        meeting.title, meeting.attendees
    );
    assert_eq!(meeting.duration_minutes, None);
    Ok(())
}
