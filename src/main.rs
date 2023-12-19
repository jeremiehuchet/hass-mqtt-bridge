use std::time::Duration;

use clap::Parser;
use hass_mqtt_autodiscovery::HomeAssistantMqtt;
use log::{debug, error, info};
use misc::app_infos;
use rika::RikaWatcher;
use rumqttc::v5::{mqttbytes::v5::Packet, AsyncClient, Event, EventLoop, MqttOptions};
use tokio_cron_scheduler::{Job, JobScheduler, JobSchedulerError};
use url::Url;

mod misc;
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

fn build_mqtt_client(
    broker_url: Url,
    username: String,
    password: String,
) -> (AsyncClient, EventLoop) {
    let options = MqttOptions::new(
        format!("{}", app_infos::name()),
        broker_url
            .host()
            .expect("A broker URL with a host")
            .to_string(),
        broker_url.port().expect("A broker URL with a port"),
    )
    .set_credentials(username, password)
    .clone();
    AsyncClient::new(options, 10)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), JobSchedulerError> {
    env_logger::init();

    let mut sched = JobScheduler::new().await.unwrap();

    let cli: Cli = Parser::parse();

    let (mqtt_client, mut mqtt_event_loop) =
        build_mqtt_client(cli.mqtt_broker_url, cli.mqtt_username, cli.mqtt_password);
    let ha_mqtt = HomeAssistantMqtt::new(mqtt_client, "homeassistant/");

    tokio::spawn(async move {
        loop {
            match mqtt_event_loop.poll().await {
                Ok(Event::Incoming(Packet::ConnAck(conn_ack))) => {
                    info!("MQTT ConnAck: {conn_ack:?}");
                }
                Ok(event) => info!("MQTT event: {event:?}"),
                Err(e) => error!("MQTT error: {e}"),
            }
        }
    });

    match (cli.rika_username, cli.rika_password) {
        (Some(username), Some(password)) => {
            info!("Scheduling Rika Firenet updates");
            let rika = RikaWatcher::new(cli.rika_baseurl, username, password, ha_mqtt.clone());
            sched
                .add(
                    Job::new_repeated_async(Duration::from_secs(10), move |_uuid, mut _l| {
                        let mut rika = rika.clone();
                        Box::pin(async move {
                            debug!("Execute Rika Firenet update");
                            rika.execute().await;
                        })
                    })
                    .unwrap(),
                )
                .await
                .unwrap();
        }
        (_, _) => debug!("No configuration for Rika Firenet"),
    }

    sched.start().await
}
