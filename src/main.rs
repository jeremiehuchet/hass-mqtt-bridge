use std::ops::RangeInclusive;
use std::time::Duration;

use actix::Actor;
use actix_web::{middleware::Logger, App, HttpServer};
use anyhow::Result;
use clap::Parser;
use log::{debug, info};
use misc::app_infos;
use misc::SuffixStrip;
use mqtt::MqttActor;
use rika::StoveDiscoveryActor;
use rika::StoveDiscoveryActorConfiguration;
use rika_firenet_client::RikaFirenetClientBuilder;
use somfy_protect::SomfyActor;
use somfy_protect_client::client::SomfyProtectClientBuilder;
use url::Url;

mod cli;
mod misc;
mod mqtt;
mod repeat;
mod rika;
mod somfy_protect;

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
    #[clap(long, env)]
    rika_baseurl: Option<Url>,

    /// Rika username
    #[clap(long, env, requires = "rika_password")]
    rika_username: Option<String>,

    /// Rika password
    #[clap(long, env, requires = "rika_username")]
    rika_password: Option<String>,

    /// Rika stove discovery scan interval
    #[clap(long, env, value_parser = cli::parse_time_delta_range, default_value = "6d..8d")]
    rika_stove_discovery_repeat_interval: RangeInclusive<Duration>,

    /// Rika stove discovery exponential backoff ceil
    #[clap(long, env, value_parser = cli::parse_time_delta, default_value = "8h")]
    rika_stove_discovery_backoff_ceil: Duration,

    /// Rika stove status update interval
    #[clap(long, env, value_parser = cli::parse_time_delta_range, default_value = "8m..12m")]
    rika_stove_status_repeat_interval: RangeInclusive<Duration>,

    /// Rika stove status update exponential backoff ceil
    #[clap(long, env, value_parser = cli::parse_time_delta, default_value = "8h")]
    rika_stove_status_backoff_ceil: Duration,

    /// Somfy Protect API base URL
    #[clap(long, env)]
    somfy_api_baseurl: Option<Url>,

    /// Somfy Protect Auth base URL
    #[clap(long, env)]
    somfy_auth_baseurl: Option<Url>,

    /// Somfy Protect API OAuth client identifier
    #[clap(
        long,
        env,
        requires = "somfy_client_secret",
        requires = "somfy_username",
        requires = "somfy_password"
    )]
    somfy_client_id: Option<String>,

    /// Somfy Protect API OAuth Client secret
    #[clap(
        long,
        env,
        requires = "somfy_client_id",
        requires = "somfy_username",
        requires = "somfy_password"
    )]
    somfy_client_secret: Option<String>,

    /// Somfy Protect API account username
    #[clap(
        long,
        env,
        requires = "somfy_client_id",
        requires = "somfy_client_secret",
        requires = "somfy_password"
    )]
    somfy_username: Option<String>,

    /// Somfy Protect API account password
    #[clap(
        long,
        env,
        requires = "somfy_client_id",
        requires = "somfy_client_secret",
        requires = "somfy_username"
    )]
    somfy_password: Option<String>,
}

impl From<&Cli> for StoveDiscoveryActorConfiguration {
    fn from(value: &Cli) -> Self {
        Self {
            stove_discovery_repeat_interval: value.rika_stove_discovery_repeat_interval.clone(),
            stove_discovery_backoff_ceil: value.rika_stove_discovery_backoff_ceil,
            stove_status_repeat_interval: value.rika_stove_status_repeat_interval.clone(),
            stove_status_backoff_ceil: value.rika_stove_status_backoff_ceil,
        }
    }
}

#[actix_web::main]
async fn main() -> Result<()> {
    env_logger::init();

    let cli: Cli = Parser::parse();

    let mqtt = MqttActor::new(&cli.mqtt_broker_url, &cli.mqtt_username, &cli.mqtt_password);
    let mqtt_addr = mqtt.start();

    match (&cli.rika_username, &cli.rika_password) {
        (Some(username), Some(password)) => {
            let mut client_builder =
                RikaFirenetClientBuilder::default().credentials(username, password);
            if let Some(base_url) = &cli.rika_baseurl {
                client_builder = client_builder.base_url(base_url.strip_repeated_suffix("/"));
            }
            let rika = StoveDiscoveryActor::new(&cli, mqtt_addr.clone(), client_builder.build());
            rika.start();
        }
        (_, _) => debug!("No configuration for Rika Firenet"),
    }

    match (
        cli.somfy_client_id,
        cli.somfy_client_secret,
        cli.somfy_username,
        cli.somfy_password,
    ) {
        (Some(client_id), Some(client_secret), Some(username), Some(password)) => {
            let mut client_builder = SomfyProtectClientBuilder::default()
                .with_client_credentials(client_id, client_secret)
                .with_user_credentials(username, password);
            if let Some(api_base_url) = cli.somfy_api_baseurl {
                client_builder =
                    client_builder.with_api_base_url(api_base_url.strip_repeated_suffix("/"));
            }
            if let Some(auth_base_url) = cli.somfy_auth_baseurl {
                client_builder =
                    client_builder.with_auth_base_url(auth_base_url.strip_repeated_suffix("/"));
            }
            let somfy = SomfyActor::new(mqtt_addr, client_builder.build());
            somfy.start();
        }
        (_, _, _, _) => debug!("No configuration for Somfy Protect"),
    }

    info!("{} version {}", app_infos::name(), app_infos::version());

    HttpServer::new(move || App::new().wrap(Logger::default()))
        .bind("127.0.0.1:8080")?
        .run()
        .await?;

    Ok(())
}
