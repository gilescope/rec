use demo::compute;

fn main() {
    // Loop so the integration test has time to attach + patch + observe.
    // Real e2e wiring lands when test-encs-e2e graduates from WIP.
    for _ in 0..3 {
        println!("compute = {}", compute());
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}
