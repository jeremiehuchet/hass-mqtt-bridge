import { Browser } from "@playwright/test";
import {
  DockerComposeEnvironment,
  StartedDockerComposeEnvironment,
  Wait,
} from "testcontainers";

export type EnvironmentStatus = {
  stack: StartedDockerComposeEnvironment;
  registeredEntities: string[];
};

const environment = new DockerComposeEnvironment(".", "docker-compose.yml")
  .withWaitStrategy(
    "homeassistant-1",
    Wait.forLogMessage(
      "INFO (MainThread) [homeassistant.core] Starting Home Assistant",
    ).withStartupTimeout(10000),
  )
  .withWaitStrategy(
    "mosquitto-1",
    Wait.forLogMessage(/mosquitto version \S+ running/).withStartupTimeout(
      5000,
    ),
  )
  .withWaitStrategy(
    "rika-firenet-mock-1",
    Wait.forLogMessage(
      "Rika Firenet mock listening on port 3000",
    ).withStartupTimeout(5000),
  )
  .withWaitStrategy(
    "somfy-protect-mock-1",
    Wait.forLogMessage(
      "Somfy Protect API mock listening on port 3000",
    ).withStartupTimeout(5000),
  );

export async function startAndInitializeStack() {
  console.log("starting stack");
  const stack = await environment.up();
  console.log("stack started");

  [
    "homeassistant-1",
    "hass-mqtt-bridge-1",
    "rika-firenet-mock-1",
    "somfy-protect-mock-1",
    "mosquitto-debug-1",
  ].forEach((containerName) => captureLogOutput(stack, containerName));

  const homeAssistantStatus: EnvironmentStatus = {
    stack,
    registeredEntities: [],
  };

  const logs = await stack.getContainer("homeassistant-1").logs();
  logs.on("data", (line: string) => {
    const parts =
      /INFO \(MainThread\) \[homeassistant.helpers.entity_registry\] Registered new (\S+) entity: (\S+)/.exec(
        line,
      );
    if (parts) {
      const entityName = parts[2];
      if (
        entityName &&
        !homeAssistantStatus.registeredEntities.includes(entityName)
      ) {
        homeAssistantStatus.registeredEntities.push(entityName);
      }
    }
  });
  return homeAssistantStatus;
}

async function captureLogOutput(
  stack: StartedDockerComposeEnvironment,
  containerName: string,
) {
  const logs = await stack.getContainer(containerName).logs();
  logs.on("data", (line: string) =>
    console.log(`${containerName} | ` + line.replace(/\n$/, "")),
  );
}
