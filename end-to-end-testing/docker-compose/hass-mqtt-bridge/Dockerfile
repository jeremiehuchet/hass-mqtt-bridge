FROM rust:slim@sha256:b8e062b1c3a80183df83d9ba187cadd2b5ab84bc78d453dbfeb2fccd5f982247 AS base
RUN apt update && apt install -y libssl-dev pkg-config

FROM base AS intermediate-builder
RUN cargo new --bin hass-mqtt-bridge
WORKDIR /hass-mqtt-bridge
COPY Cargo.toml Cargo.lock ./
RUN cat - <<EOF > src/main.rs
fn main() {
    println!("Simple program to prebuild dependencies");
}
EOF
RUN cargo build --release

FROM intermediate-builder AS builder
COPY src /tmp/src
RUN cp /tmp/src/* src/
RUN cargo build --release

FROM base
COPY --from=builder /hass-mqtt-bridge/target/release/hass-mqtt-bridge /usr/local/bin
ENTRYPOINT ["hass-mqtt-bridge"]