use instructor_ai::{model, BlockingClient};

#[model]
struct Assignment {
    owner: String,
    task: String,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let model = std::env::args().nth(1).ok_or("pass provider/model")?;
    let client = BlockingClient::from_provider(&model)?;
    let prompt = "Extract each assignment. Ali will book the hall. Bo will print the agenda.";
    for assignment in client.create_iterable::<Assignment>(prompt)? {
        let assignment = assignment?;
        println!("{} will {}", assignment.owner, assignment.task);
    }
    Ok(())
}
