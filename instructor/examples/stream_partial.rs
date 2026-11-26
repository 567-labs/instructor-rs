use futures_util::StreamExt;
use instructor_ai::{model, Client, Partial, PartialEvent};

#[model]
struct Venue {
    city: String,
}

#[model]
struct Meeting {
    title: String,
    venue: Venue,
}

fn preview(snapshot: Partial<Meeting>) {
    let title = snapshot.title.as_deref().unwrap_or("…");
    let city = snapshot.venue.and_then(|venue| venue.city);
    println!("{title} in {}", city.as_deref().unwrap_or("…"));
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let model = std::env::args().nth(1).ok_or("pass provider/model")?;
    let client = Client::from_provider(&model)?;
    let mut updates = client
        .create_partial::<Meeting>("Extract the meeting. The Annual Review takes place in Oslo.")
        .await?;
    while let Some(update) = updates.next().await {
        match update? {
            PartialEvent::Partial(snapshot) => preview(snapshot),
            PartialEvent::Complete(meeting) => {
                println!("Confirmed {} in {}", meeting.title, meeting.venue.city);
            }
        }
    }
    Ok(())
}
