use actix::{Actor, System};
use actix_web::{middleware::Logger, App, HttpServer};
use anyhow::Result;
use clap::Parser;
use log::{debug, error, info};
use mqtt::MqttActor;
use rika::RikaActor;
use url::Url;

mod misc;
mod mqtt;
mod rika;

#[derive(Parser)]
struct Cli {
    /// MQTT broker URL
    #[clap(long, env, default_value_t = Url::parse("mqtt://localhost:1883").expect("A valid broker URL example"))]
    mqtt_broker_url: Url,

    /// MQTT username
    #[clap(long, env)]
    mqtt_username: String,

    /// MQTT password
    #[clap(long, env)]
    mqtt_password: String,

    /// Rika API base URL
    #[clap(long, env, default_value_t = Url::parse("https://www.rika-firenet.com").expect("A valid URL"))]
    rika_baseurl: Url,

    /// Rika username
    #[clap(long, env)]
    rika_username: Option<String>,

    /// Rika password
    #[clap(long, env)]
    rika_password: Option<String>,
}

#[actix_web::main]
async fn main() -> Result<()> {
    env_logger::init();

    let cli: Cli = Parser::parse();

    let mqtt = MqttActor::new(cli.mqtt_broker_url, cli.mqtt_username, cli.mqtt_password);
    let mqtt_addr = mqtt.start();

    match (cli.rika_username, cli.rika_password) {
        (Some(username), Some(password)) => {
            let rika = RikaActor::new(mqtt_addr, cli.rika_baseurl, username, password);
            rika.start();
        }
        (_, _) => debug!("No configuration for Rika Firenet"),
    }

    HttpServer::new(move || App::new().wrap(Logger::default()))
        .bind("127.0.0.1:8080")?
        .run()
        .await?;
    Ok(())
}
