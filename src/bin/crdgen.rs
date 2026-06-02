//! Generate the CRD YAML: `cargo run --bin crdgen > deploy/crd.yaml`
use kube::CustomResourceExt;
use rathole_operator::crd::RatholeConfiguration;

fn main() {
    print!(
        "{}",
        serde_yaml::to_string(&RatholeConfiguration::crd()).unwrap()
    );
}
