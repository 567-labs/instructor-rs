use instructor_ai::{distillation::Instructions, model, Error};

#[model]
struct Product {
    value: i64,
}

fn main() -> Result<(), Error> {
    let task = Instructions::<(i32, i32), Product>::new(
        "multiply",
        "fn multiply(a: i32, b: i32) -> Product\nReturn the product of two integers.",
    );
    let mut output = std::io::stdout().lock();
    for input in [(123, 456), (205, 312), (900, 101)] {
        task.distil(&input, &mut output, |&(a, b)| {
            Ok(Product {
                value: i64::from(a) * i64::from(b),
            })
        })?;
    }
    Ok(())
}
