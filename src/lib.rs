//! rathole-operator — a rathole-backed Kubernetes LoadBalancer controller.
//!
//! A cluster-scoped [`crd::RatholeConfiguration`] describes a tunnel backend (a
//! rathole server on a public VPS). Services of `type: LoadBalancer` with
//! `loadBalancerClass: tatupesonen.rathole/tunnel` are exposed through it: the operator
//! renders the rathole client config (and optionally pushes the server config to
//! the VPS), runs the rathole client Deployment, and writes the VPS address into
//! each Service's `status.loadBalancer.ingress`.

pub mod controller;
pub mod crd;
pub mod error;
pub mod lb;
pub mod push;
pub mod resources;

pub use error::{Error, Result};
