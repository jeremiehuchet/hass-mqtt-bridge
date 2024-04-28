import {
  DockerComposeEnvironment,
  StartedDockerComposeEnvironment,
  Wait,
} from "testcontainers";

const dockerCompose = new DockerComposeEnvironment(".", "docker-compose.yml")
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
  )
  .withWaitStrategy(
    "hass-mqtt-bridge-1",
    Wait.forLogMessage(
      "Actix runtime found; starting in Actix runtime",
    ).withStartupTimeout(5000),
  );

export class TestPlatform {
  environment: StartedDockerComposeEnvironment | null;
  readonly mosquittoMessages: string[] = [];
  readonly registeredEntities: string[] = [];

  constructor() {}

  async up() {
    const env = await dockerCompose.up();
    const haLogs = await env.getContainer("homeassistant-1").logs();
    haLogs.on("data", (line: string) => {
      const parts =
        /INFO \(MainThread\) \[homeassistant.helpers.entity_registry\] Registered new (\S+) entity: ([a-zA-Z0-9-_.]+)/.exec(
          line.trim(),
        );
      if (parts) {
        const entityName = parts[2];
        if (entityName && !this.registeredEntities.includes(entityName)) {
          this.registeredEntities.push(entityName);
        }
      }
    });
    const moquittoDebugLogs = await env
      .getContainer("mosquitto-debug-1")
      .logs();
    moquittoDebugLogs.on("data", (line: string) => {
      this.mosquittoMessages.push(line.trim());
    });
    this.environment = env;
  }

  async down() {
    await this.environment?.down({
      removeVolumes: true,
    });
    this.environment = null;
    this.registeredEntities.length = 0;
  }

  countRegisteredEntities(pattern: RegExp): number {
    return this.registeredEntities.filter((name) => pattern.test(name)).length;
  }

  getMosquittoMessages(): string[] {
    return [...this.mosquittoMessages];
  }
}
