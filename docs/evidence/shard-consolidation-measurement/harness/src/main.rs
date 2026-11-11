use std::path::Path;
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let result = match args.as_slice() {
        [_, mode, root] if mode == "prepare" => {
            shard_measurement::prepare_child_fixture(Path::new(root))
        }
        [_, mode, root, phase] if mode == "child" => match tokio::runtime::Runtime::new() {
            Ok(runtime) => runtime.block_on(shard_measurement::owned_child(Path::new(root), phase)),
            Err(error) => Err(shard_measurement::MeasurementError::Engine(
                error.to_string(),
            )),
        },
        _ => Err(shard_measurement::MeasurementError::Containment(
            "usage: prepare ROOT | child ROOT {shard-before-registry|aux-one-before-two}".into(),
        )),
    };
    if let Err(error) = result {
        eprintln!("{error:?}");
        std::process::exit(1)
    }
}
