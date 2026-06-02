# Builds both the operator and the config receiver into one image.
FROM rust:1-slim-bookworm AS build
WORKDIR /src
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config && rm -rf /var/lib/apt/lists/*
# Cache dependencies.
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src/bin \
 && echo 'fn main(){}' > src/main.rs \
 && echo '' > src/lib.rs \
 && echo 'fn main(){}' > src/bin/crdgen.rs \
 && echo 'fn main(){}' > src/bin/receiver.rs \
 && cargo build --release --bins || true
# Real sources.
COPY . .
RUN touch src/main.rs src/lib.rs \
 && cargo build --release --bin rathole-operator --bin rathole-config-receiver

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /src/target/release/rathole-operator /usr/local/bin/rathole-operator
COPY --from=build /src/target/release/rathole-config-receiver /usr/local/bin/rathole-config-receiver
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/rathole-operator"]
