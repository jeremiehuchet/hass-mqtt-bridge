FROM rust:slim
RUN apt update && apt install -y libssl-dev pkg-config
RUN cargo new --bin hass-mqtt-bridge

WORKDIR /hass-mqtt-bridge
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo install --path . --root /usr/local

ENTRYPOINT ["hass-mqtt-bridge"]