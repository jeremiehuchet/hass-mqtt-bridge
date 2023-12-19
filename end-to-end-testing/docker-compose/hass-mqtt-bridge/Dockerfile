FROM rust:slim
RUN apt update && apt install -y libssl-dev pkg-config
RUN cargo new --bin hass-mqtt-bridge

WORKDIR /hass-mqtt-bridge
COPY Cargo.toml Cargo.lock ./
RUN cargo build --release
RUN rm -rf src target/release

COPY src ./src
RUN cargo build --release
RUN cargo install --path . --root /usr/local

ENTRYPOINT ["hass-mqtt-bridge"]