use instructor_ai::{model, BlockingClient, ParallelTools};

#[model]
/// Look up the forecast for a city.
struct Forecast {
    city: String,
}

#[model]
/// Search for a place or event.
struct Search {
    query: String,
}

#[derive(ParallelTools)]
enum Action {
    Forecast(Forecast),
    Search(Search),
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let model = std::env::args().nth(1).ok_or("pass provider/model")?;
    let client = BlockingClient::from_provider(&model)?;
    let actions: Vec<Action> = client
        .create_parallel("What is the weather in Oslo and Bergen? Look up museums in Oslo too.")?;
    for action in actions {
        match action {
            Action::Forecast(forecast) => println!("Forecast for {}", forecast.city),
            Action::Search(search) => println!("Search for {}", search.query),
        }
    }
    Ok(())
}
