use actix::prelude::*;
use actix_web::rt::time;
use async_stream::stream;
use ha_mqtt_discovery::v5::{
    mqttbytes::{
        v5::{ConnAck, Packet, Publish},
        QoS,
    },
    AsyncClient, ClientError, Event, MqttOptions,
};
use ha_mqtt_discovery::{Entity, HomeAssistantMqtt};
use log::{error, info, trace};
use serde::Serialize;
use serde_json::Value;
use std::{collections::HashSet, time::Duration};
use url::Url;

use crate::misc::{app_infos, hostname, HumanReadable};

const BIRTH_LAST_WILL_TOPIC: &str = "homeassistant/status";
const BIRTH_PAYLOAD: &str = "online";
const LAST_WILL_PAYLOAD: &str = "offline";

pub struct MqttActor {
    mqtt_options: MqttOptions,
    mqtt_client: Option<AsyncClient>,
    ha_mqtt: Option<HomeAssistantMqtt>,
    listeners: HashSet<Recipient<MqttMessage>>,
}

impl MqttActor {
    pub fn new(broker_url: &Url, username: &String, password: &String) -> Self {
        let mqtt_options = MqttOptions::new(
            format!("{}@{}", app_infos::name(), hostname()),
            broker_url
                .host()
                .expect("A broker URL with a host")
                .to_string(),
            broker_url.port().expect("A broker URL with a port"),
        )
        .set_credentials(username, password)
        .clone();
        MqttActor {
            mqtt_options,
            mqtt_client: None,
            ha_mqtt: None,
            listeners: HashSet::new(),
        }
    }

    fn subscribe_ha_events(&self, ctx: &mut Context<Self>, ack: ConnAck) {
        if let Some(client) = self.mqtt_client.clone() {
            async move {
                let _ = client
                    .subscribe(BIRTH_LAST_WILL_TOPIC, QoS::AtLeastOnce)
                    .await;
            }
            .into_actor(self)
            .spawn(ctx);
        }
    }

    fn handle_event(&self, event: Event) {
        trace!("event from server: {event:?}");
        match event {
            Event::Incoming(Packet::Publish(publish)) => {
                let message = MqttMessage::from(publish);
                for recipient in &self.listeners {
                    recipient.do_send(message.clone());
                }
            }
            _ => {}
        }
    }
}

impl Actor for MqttActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        let (async_client, mut event_loop) = AsyncClient::new(self.mqtt_options.clone(), 10);
        self.mqtt_client = Some(async_client.clone());
        self.ha_mqtt = Some(HomeAssistantMqtt::new(async_client, "homeassistant/"));

        ctx.add_stream(stream! {
            let backoff = exponential_backoff::Backoff::new(u32::MAX, Duration::from_millis(50), Duration::from_secs(300));
            let mut backoff_session = backoff.iter();
            loop {
                match event_loop.poll().await {
                    Ok(event) => yield event,
                    Err(connection_error) => {
                        let delay = match   backoff_session.next() {
                            Some(Some(delay)) => delay,
                            _ => Duration::from_secs(300),
                        };
                        error!("Backing off for {}: {connection_error} (see also MQTT server logs)", delay.prettify());
                        time::sleep(delay).await;
                    }
                }
            }
        });
    }
}

impl StreamHandler<Event> for MqttActor {
    fn handle(&mut self, msg: Event, ctx: &mut Self::Context) {
        match msg {
            Event::Incoming(Packet::ConnAck(ack)) => {
                self.subscribe_ha_events(ctx, ack);
            }
            event => self.handle_event(event),
        }
    }

    fn finished(&mut self, ctx: &mut Self::Context) {
        info!("Stopped listening MQTT events");
    }
}

#[derive(Message, Clone)]
#[rtype(result = "()")]
pub struct EntityConfiguration(pub Entity);

impl Handler<EntityConfiguration> for MqttActor {
    type Result = ();

    fn handle(&mut self, msg: EntityConfiguration, ctx: &mut Self::Context) -> Self::Result {
        if let Some(ha_mqtt) = self.ha_mqtt.clone() {
            async move {
                let result = ha_mqtt.publish_entity(msg.0).await;
                if let Err(error) = result {
                    error!("Unable to publish entity: {error}")
                }
            }
            .into_actor(self)
            .spawn(ctx);
        } else {
            error!("MQTT client not available")
        }
    }
}

#[derive(Message, Clone)]
#[rtype(result = "()")]
pub struct PublishEntityData {
    topic: String,
    payload: Value,
}

impl PublishEntityData {
    pub fn new<S: Serialize>(topic: String, payload: S) -> Self {
        PublishEntityData {
            topic,
            payload: serde_json::to_value(payload).unwrap_or_default(),
        }
    }
}

impl Handler<PublishEntityData> for MqttActor {
    type Result = ();

    fn handle(&mut self, msg: PublishEntityData, ctx: &mut Self::Context) -> Self::Result {
        match self.ha_mqtt.clone() {
            Some(ha_mqtt) => {
                let msg = msg.clone();
                async move {
                    let result = ha_mqtt.publish_data(&msg.topic, &msg.payload, None).await;
                    if let Err(error) = result {
                        error!("Unable to publish data: {error}")
                    }
                }
                .into_actor(self)
                .spawn(ctx);
            }
            None => error!("MQTT client not available"),
        }
    }
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "Result<SubscribeSuccess, SubscribeError>")]
pub struct Subscribe {
    topic: String,
    recipient: Recipient<MqttMessage>,
}

impl Subscribe {
    pub fn new(topic: String, recipient: Recipient<MqttMessage>) -> Self {
        Subscribe { topic, recipient }
    }
}

#[derive(Debug)]
pub struct SubscribeSuccess {
    pub topic: String,
}

impl SubscribeSuccess {
    pub fn new(topic: String) -> Self {
        SubscribeSuccess { topic }
    }
}

#[derive(Debug)]
pub struct SubscribeError {
    pub topic: String,
    pub error: ClientError,
}

impl SubscribeError {
    pub fn new(topic: String, error: ClientError) -> Self {
        SubscribeError { topic, error }
    }
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct MqttMessage {
    pub topic: String,
    pub payload: String,
}

impl From<Publish> for MqttMessage {
    fn from(publish_event: Publish) -> Self {
        let topic = String::from_utf8_lossy(&publish_event.topic).to_string();
        let payload = String::from_utf8_lossy(&publish_event.payload).to_string();
        MqttMessage { topic, payload }
    }
}

impl Handler<Subscribe> for MqttActor {
    type Result = ResponseActFuture<Self, Result<SubscribeSuccess, SubscribeError>>;

    fn handle(&mut self, msg: Subscribe, ctx: &mut Self::Context) -> Self::Result {
        let original_msg = msg.clone();
        let mqtt_client = self.mqtt_client.clone();
        Box::pin(
            async move {
                mqtt_client
                    .unwrap()
                    .subscribe(msg.topic, QoS::AtLeastOnce)
                    .await
            }
            .into_actor(self)
            .map(|res, act, _ctx| match res {
                Ok(_) => {
                    act.listeners.insert(msg.recipient);
                    Ok(SubscribeSuccess::new(original_msg.topic))
                }
                Err(err) => Err(SubscribeError::new(original_msg.topic, err)),
            }),
        )
    }
}

pub trait HaMqttEntity<T> {
    fn list_entities(self) -> Vec<Entity>;
    fn build_payloads(&self, data: T) -> Vec<PublishEntityData>;
}
