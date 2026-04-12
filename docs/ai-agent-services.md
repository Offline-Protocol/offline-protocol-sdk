# AI Agent Services Guide (React Native)

Build AI agent-to-agent communication and distributed pipelines on the Offline Protocol mesh — no SDK changes needed. This guide defines conventions, schemas, and patterns on top of the existing `MeshServices` API.

## Overview

The Offline Protocol's service discovery and request/response system (see [Service Discovery](./service-discovery.md)) provides everything needed to build AI agent services. Agents register as services, discover each other via gossip, and exchange structured JSON messages — all offline, with no central coordinator.

This guide covers two patterns:

1. **Agent-to-agent communication** — one agent sends a task to another and receives a result.
2. **Distributed pipelines** — an orchestrator chains multiple agents into a multi-step workflow.

No Rust code changes, no binding changes. Everything here is a convention layer in TypeScript on top of the existing `MeshServices` class.

```
Agent A (consumer)                  Mesh                  Agent B (provider)
      |                               |                          |
      |  registerService('agent.qa')  |  registerService('agent.summarizer')
      |                               |                          |
      |-- discoverServices('agent.')-->|-- (gossip forward) ---->|
      |                               |                          |  (matches prefix)
      |<-- service_discovered --------|<-- service_discovered ---|
      |                               |                          |
      |-- sendServiceRequest() ------>|------------------------->|
      |     method: 'agent.task.request'        service_request_received
      |     body: { type: 'task_request', ... }                  |
      |                               |                          |  (agent processes task)
      |                               |<-- respondToServiceRequest()
      |<-- service_response_received--|     body: { type: 'task_result', ... }
```

## Agent Message Schema

Agents communicate by sending JSON in the `body` field of service requests and responses. All messages use a `type` discriminator.

### Message Types

| Type | Direction | Description |
|------|-----------|-------------|
| `task_request` | Request | Ask an agent to perform a capability |
| `task_result` | Response | Result of a completed task |
| `tool_call` | Request | Invoke a tool on another agent |
| `tool_result` | Response | Tool execution result |
| `reasoning_update` | Request | Share intermediate progress |

### TypeScript Interfaces

```typescript
// ─── Discriminated union ────────────────────────────────────

type AgentMessage =
  | TaskRequest
  | TaskResult
  | ToolCall
  | ToolResult
  | ReasoningUpdate;

// ─── Task request / result ──────────────────────────────────

interface TaskRequest {
  type: 'task_request';
  task_id: string;         // UUID — correlates request to result
  capability: string;      // What to do, e.g. 'summarize', 'translate'
  input: string;           // Primary input text
  context?: string;        // Optional additional context
  pipeline_id?: string;    // Set when part of a pipeline
  step_index?: number;     // Step position in pipeline
  timeout_ms?: number;     // Per-task timeout
}

interface TaskResult {
  type: 'task_result';
  task_id: string;         // Matches the request's task_id
  status: 'ok' | 'error' | 'timeout';
  output?: string;         // Result text (present when status is 'ok')
  error?: string;          // Error message (present when status is 'error')
  execution_ms?: number;   // How long the task took
}

// ─── Tool call / result ─────────────────────────────────────

interface ToolCall {
  type: 'tool_call';
  call_id: string;         // UUID — correlates call to result
  tool_name: string;       // Tool to invoke, e.g. 'run_code', 'search'
  arguments: Record<string, unknown>;  // Tool arguments
}

interface ToolResult {
  type: 'tool_result';
  call_id: string;         // Matches the call's call_id
  output?: string;         // Tool output
  error?: string;          // Error message if tool failed
}

// ─── Reasoning update ───────────────────────────────────────

interface ReasoningUpdate {
  type: 'reasoning_update';
  task_id: string;            // Which task this update belongs to
  step: number;               // Step counter (monotonically increasing)
  content: string;            // Update content
  update_type: 'thinking' | 'progress' | 'partial_result';
}
```

## Agent Registration

Use the `agent.<name>` convention for service IDs. The `capabilities` map encodes agent metadata that consumers can inspect before sending tasks.

```typescript
import { MeshServices } from '@offline-protocol/react-native';

const services = new MeshServices();

// Register a summarizer agent
await services.registerService('agent.summarizer', '1.0', {
  model: 'llama-3-8b',
  capabilities: 'summarize,extract_key_points',
  max_input_bytes: '32000',
  estimated_latency_ms: '2000',
});

// Register a translator agent
await services.registerService('agent.translator', '1.0', {
  model: 'nllb-200',
  capabilities: 'translate',
  languages: 'en,es,fr,de,zh,ja',
  max_input_bytes: '16000',
  estimated_latency_ms: '1500',
});
```

### Capabilities Map Conventions

| Key | Description | Example |
|-----|-------------|---------|
| `model` | Model name or identifier | `"llama-3-8b"` |
| `capabilities` | Comma-separated list of capabilities | `"summarize,translate"` |
| `languages` | Supported languages (for translation) | `"en,es,fr"` |
| `max_input_bytes` | Maximum input size accepted | `"32000"` |
| `estimated_latency_ms` | Typical response time | `"2000"` |

All values are strings (the `capabilities` map is `Record<string, string>`).

## Agent Discovery

Discover agents by prefix to find all agents, or by full ID for a specific one.

```typescript
import { OfflineProtocol, MeshServices } from '@offline-protocol/react-native';

const protocol = new OfflineProtocol({ appId: 'my-app', userId: 'node-1' });
const services = new MeshServices();

// Track discovered agents
const agents = new Map<string, {
  peerId: string;
  serviceId: string;
  version: string;
  capabilities: Record<string, string>;
  hopCount: number;
}>();

// Listen for discovery responses
protocol.on('service_discovered', (event) => {
  if (!event.service_id.startsWith('agent.')) return;

  // Filter by capability if needed
  const caps = event.capabilities.capabilities?.split(',') ?? [];
  console.log(`Found ${event.service_id} at ${event.provider_peer_id}`,
    `(${event.hop_count} hops) — capabilities: ${caps.join(', ')}`);

  // Keep the closest provider per service
  const existing = agents.get(event.service_id);
  if (!existing || event.hop_count < existing.hopCount) {
    agents.set(event.service_id, {
      peerId: event.provider_peer_id,
      serviceId: event.service_id,
      version: event.version,
      capabilities: event.capabilities,
      hopCount: event.hop_count,
    });
  }
});

// Discover all agents on the mesh
await services.discoverServices('agent.');

// Or discover a specific agent
await services.discoverServices('agent.summarizer');
```

Discovery is **eventual** — responses arrive asynchronously over the mesh. Wait a few seconds after calling `discoverServices()` before assuming all agents have been found.

## Task Request/Response

The core pattern: Agent A sends a task to Agent B via `sendServiceRequest()`, Agent B processes it and responds via `respondToServiceRequest()`.

### Sending a Task

```typescript
function generateId(): string {
  return Math.random().toString(36).slice(2) + Date.now().toString(36);
}

// Send a summarization task to a discovered agent
const agent = agents.get('agent.summarizer');
if (!agent) throw new Error('No summarizer agent discovered');

const taskId = generateId();
const body: TaskRequest = {
  type: 'task_request',
  task_id: taskId,
  capability: 'summarize',
  input: 'The Offline Protocol SDK provides an offline-first messaging...',
  timeout_ms: 10000,
};

const requestId = await services.sendServiceRequest(
  agent.peerId,
  agent.serviceId,
  'agent.task.request',
  JSON.stringify(body)
);

// Wait for the response via events (see below)
```

### Handling a Task (Provider Side)

```typescript
protocol.on('service_request_received', async (event) => {
  if (event.method !== 'agent.task.request') return;

  const request: TaskRequest = JSON.parse(event.body);
  const startTime = Date.now();

  try {
    // Process the task (your AI logic here)
    const summary = await myModel.summarize(request.input);

    const result: TaskResult = {
      type: 'task_result',
      task_id: request.task_id,
      status: 'ok',
      output: summary,
      execution_ms: Date.now() - startTime,
    };

    await services.respondToServiceRequest(
      event.request_id,
      event.sender,
      event.service_id,
      'ok',
      JSON.stringify(result)
    );
  } catch (err) {
    const result: TaskResult = {
      type: 'task_result',
      task_id: request.task_id,
      status: 'error',
      error: err instanceof Error ? err.message : String(err),
      execution_ms: Date.now() - startTime,
    };

    await services.respondToServiceRequest(
      event.request_id,
      event.sender,
      event.service_id,
      'error',
      JSON.stringify(result)
    );
  }
});
```

### Receiving a Result (Consumer Side)

```typescript
// Promise-based wrapper for request/response
function sendTask(
  services: MeshServices,
  protocol: OfflineProtocol,
  peerId: string,
  serviceId: string,
  task: TaskRequest,
  timeoutMs: number = 30000
): Promise<TaskResult> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      protocol.off('service_response_received', handler);
      reject(new Error(`Task ${task.task_id} timed out after ${timeoutMs}ms`));
    }, timeoutMs);

    const handler = (event: ServiceResponseReceivedEvent) => {
      try {
        const result: TaskResult = JSON.parse(event.body);
        if (result.task_id !== task.task_id) return; // Not our response
        clearTimeout(timer);
        protocol.off('service_response_received', handler);
        resolve(result);
      } catch {
        // Ignore parse errors from unrelated responses
      }
    };
    protocol.on('service_response_received', handler);

    services.sendServiceRequest(
      peerId, serviceId, 'agent.task.request', JSON.stringify(task)
    ).catch((err) => {
      clearTimeout(timer);
      protocol.off('service_response_received', handler);
      reject(err);
    });
  });
}

// Usage
const result = await sendTask(services, protocol, agent.peerId, agent.serviceId, {
  type: 'task_request',
  task_id: generateId(),
  capability: 'summarize',
  input: 'Long text to summarize...',
});

if (result.status === 'ok') {
  console.log('Summary:', result.output);
} else {
  console.error('Task failed:', result.error);
}
```

## Tool Calling Between Agents

An agent can delegate tool execution to another agent using the same request/response pattern with `agent.tool.call` as the method.

### Caller (Reasoning Agent)

```typescript
const codeAgent = agents.get('agent.code-executor');
if (!codeAgent) throw new Error('No code execution agent found');

const callId = generateId();
const toolCall: ToolCall = {
  type: 'tool_call',
  call_id: callId,
  tool_name: 'run_python',
  arguments: {
    code: 'print(sum(range(100)))',
    timeout_secs: 5,
  },
};

const requestId = await services.sendServiceRequest(
  codeAgent.peerId,
  codeAgent.serviceId,
  'agent.tool.call',
  JSON.stringify(toolCall)
);
```

### Handler (Tool Agent)

```typescript
protocol.on('service_request_received', async (event) => {
  if (event.method !== 'agent.tool.call') return;

  const call: ToolCall = JSON.parse(event.body);

  try {
    const output = await executeTool(call.tool_name, call.arguments);
    const result: ToolResult = {
      type: 'tool_result',
      call_id: call.call_id,
      output,
    };

    await services.respondToServiceRequest(
      event.request_id, event.sender, event.service_id,
      'ok', JSON.stringify(result)
    );
  } catch (err) {
    const result: ToolResult = {
      type: 'tool_result',
      call_id: call.call_id,
      error: err instanceof Error ? err.message : String(err),
    };

    await services.respondToServiceRequest(
      event.request_id, event.sender, event.service_id,
      'error', JSON.stringify(result)
    );
  }
});
```

## Distributed Pipeline Orchestration

Pipelines chain multiple agents in sequence, parallel, or conditionally. This is implemented entirely in app-layer TypeScript — no SDK pipeline engine needed.

### Pipeline Definition

```typescript
interface PipelineStep {
  name: string;
  capability: string;
  serviceId?: string;   // Specific agent, or auto-resolve from capability
  inputTransform?: (previousOutput: string) => string;
  condition?: (previousOutput: string) => boolean;
  timeout_ms?: number;
}

interface Pipeline {
  id: string;
  steps: PipelineStep[];
}
```

### PipelineOrchestrator

```typescript
class PipelineOrchestrator {
  constructor(
    private services: MeshServices,
    private protocol: OfflineProtocol,
    private agents: Map<string, { peerId: string; serviceId: string; capabilities: Record<string, string> }>,
  ) {}

  /** Find an agent that supports the given capability. */
  private findAgent(capability: string, preferredServiceId?: string) {
    if (preferredServiceId) {
      const agent = this.agents.get(preferredServiceId);
      if (agent) return agent;
    }
    // Search all agents for matching capability
    for (const [, agent] of this.agents) {
      const caps = agent.capabilities.capabilities?.split(',') ?? [];
      if (caps.includes(capability)) return agent;
    }
    return null;
  }

  /** Run a pipeline sequentially: step N output feeds step N+1 input. */
  async runSequential(pipeline: Pipeline, initialInput: string): Promise<string> {
    let currentOutput = initialInput;

    for (let i = 0; i < pipeline.steps.length; i++) {
      const step = pipeline.steps[i];

      // Check condition
      if (step.condition && !step.condition(currentOutput)) {
        console.log(`Pipeline ${pipeline.id}: skipping step "${step.name}" (condition false)`);
        continue;
      }

      const agent = this.findAgent(step.capability, step.serviceId);
      if (!agent) throw new Error(`No agent found for capability "${step.capability}"`);

      // Transform input if needed
      const input = step.inputTransform
        ? step.inputTransform(currentOutput)
        : currentOutput;

      const task: TaskRequest = {
        type: 'task_request',
        task_id: generateId(),
        capability: step.capability,
        input,
        pipeline_id: pipeline.id,
        step_index: i,
        timeout_ms: step.timeout_ms,
      };

      console.log(`Pipeline ${pipeline.id}: step ${i} "${step.name}" → ${agent.serviceId}`);

      const result = await sendTask(
        this.services, this.protocol,
        agent.peerId, agent.serviceId,
        task, step.timeout_ms ?? 30000
      );

      if (result.status !== 'ok') {
        throw new Error(
          `Pipeline ${pipeline.id} failed at step "${step.name}": ${result.error}`
        );
      }

      currentOutput = result.output ?? '';
    }

    return currentOutput;
  }

  /** Run multiple steps in parallel, returning all results. */
  async runParallel(
    pipelineId: string,
    steps: PipelineStep[],
    input: string
  ): Promise<string[]> {
    const promises = steps.map(async (step) => {
      const agent = this.findAgent(step.capability, step.serviceId);
      if (!agent) throw new Error(`No agent found for capability "${step.capability}"`);

      const taskInput = step.inputTransform ? step.inputTransform(input) : input;
      const task: TaskRequest = {
        type: 'task_request',
        task_id: generateId(),
        capability: step.capability,
        input: taskInput,
        pipeline_id: pipelineId,
        timeout_ms: step.timeout_ms,
      };

      const result = await sendTask(
        this.services, this.protocol,
        agent.peerId, agent.serviceId,
        task, step.timeout_ms ?? 30000
      );

      if (result.status !== 'ok') {
        throw new Error(`Parallel step "${step.name}" failed: ${result.error}`);
      }

      return result.output ?? '';
    });

    return Promise.all(promises);
  }
}
```

### Example: 3-Step Pipeline

```typescript
const pipeline: Pipeline = {
  id: generateId(),
  steps: [
    {
      name: 'transcribe',
      capability: 'transcribe',
      serviceId: 'agent.transcriber',
      timeout_ms: 15000,
    },
    {
      name: 'summarize',
      capability: 'summarize',
      serviceId: 'agent.summarizer',
      timeout_ms: 10000,
    },
    {
      name: 'translate',
      capability: 'translate',
      serviceId: 'agent.translator',
      timeout_ms: 8000,
      inputTransform: (summary) =>
        JSON.stringify({ text: summary, target_lang: 'es' }),
    },
  ],
};

const orchestrator = new PipelineOrchestrator(services, protocol, agents);
const spanishSummary = await orchestrator.runSequential(pipeline, audioTranscript);
console.log('Result:', spanishSummary);
```

## Reasoning Updates

For long-running tasks, agents can send incremental updates as separate service requests back to the consumer. Since the protocol has no streaming, this is the way to share intermediate state.

```typescript
// Provider: send reasoning updates during task execution
async function processWithUpdates(
  services: MeshServices,
  requesterPeerId: string,
  task: TaskRequest,
) {
  // Send a progress update back to the requester
  const sendUpdate = async (step: number, content: string) => {
    const update: ReasoningUpdate = {
      type: 'reasoning_update',
      task_id: task.task_id,
      step,
      content,
      update_type: 'progress',
    };
    // Send as a new service request to the consumer
    await services.sendServiceRequest(
      requesterPeerId,
      'agent.updates',  // Consumer registers this service to receive updates
      'agent.reasoning.update',
      JSON.stringify(update)
    );
  };

  await sendUpdate(1, 'Analyzing input text...');
  const analysis = await analyzeText(task.input);

  await sendUpdate(2, 'Generating summary...');
  const summary = await generateSummary(analysis);

  return summary;
}

// Consumer: listen for reasoning updates
protocol.on('service_request_received', (event) => {
  if (event.method === 'agent.reasoning.update') {
    const update: ReasoningUpdate = JSON.parse(event.body);
    console.log(`[Task ${update.task_id}] Step ${update.step}: ${update.content}`);

    // Acknowledge the update
    services.respondToServiceRequest(
      event.request_id, event.sender, event.service_id,
      'ok', '{}'
    );
  }
});

// Register to receive updates
await services.registerService('agent.updates', '1.0', {});
```

## Complete Working Example

End-to-end example: initialize the protocol, register an agent, discover remote agents, send a task, and run a pipeline.

```typescript
import { OfflineProtocol, MeshServices } from '@offline-protocol/react-native';
import type { ServiceDiscoveredEvent, ServiceResponseReceivedEvent } from '@offline-protocol/react-native';

// ─── Helpers ────────────────────────────────────────────────

function generateId(): string {
  return Math.random().toString(36).slice(2) + Date.now().toString(36);
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

// ─── AgentClient ────────────────────────────────────────────

class AgentClient {
  private agents = new Map<string, {
    peerId: string;
    serviceId: string;
    version: string;
    capabilities: Record<string, string>;
    hopCount: number;
  }>();

  constructor(
    private services: MeshServices,
    private protocol: OfflineProtocol,
  ) {
    this.protocol.on('service_discovered', (event: ServiceDiscoveredEvent) => {
      if (!event.service_id.startsWith('agent.')) return;
      const existing = this.agents.get(event.service_id);
      if (!existing || event.hop_count < existing.hopCount) {
        this.agents.set(event.service_id, {
          peerId: event.provider_peer_id,
          serviceId: event.service_id,
          version: event.version,
          capabilities: event.capabilities,
          hopCount: event.hop_count,
        });
      }
    });
  }

  /** Discover agents and wait for responses. */
  async discover(prefix: string = 'agent.', waitMs: number = 3000) {
    await this.services.discoverServices(prefix);
    await delay(waitMs);
    return new Map(this.agents);
  }

  /** Send a task and wait for the result. */
  async sendTask(serviceId: string, task: TaskRequest, timeoutMs = 30000): Promise<TaskResult> {
    const agent = this.agents.get(serviceId);
    if (!agent) throw new Error(`Agent "${serviceId}" not discovered`);

    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.protocol.off('service_response_received', handler);
        reject(new Error(`Task ${task.task_id} timed out`));
      }, timeoutMs);

      const handler = (event: ServiceResponseReceivedEvent) => {
        try {
          const result: TaskResult = JSON.parse(event.body);
          if (result.task_id !== task.task_id) return;
          clearTimeout(timer);
          this.protocol.off('service_response_received', handler);
          resolve(result);
        } catch { /* ignore unrelated responses */ }
      };
      this.protocol.on('service_response_received', handler);

      this.services.sendServiceRequest(
        agent.peerId, agent.serviceId, 'agent.task.request', JSON.stringify(task)
      ).catch((err) => {
        clearTimeout(timer);
        this.protocol.off('service_response_received', handler);
        reject(err);
      });
    });
  }

  getAgent(serviceId: string) {
    return this.agents.get(serviceId) ?? null;
  }
}

// ─── Main ───────────────────────────────────────────────────

async function main() {
  // 1. Initialize protocol
  const protocol = new OfflineProtocol({
    appId: 'agent-demo',
    userId: 'node-1',
    encryption: { requireEncryption: false },
  });
  await protocol.start();
  const services = new MeshServices();

  // 2. Register a local agent
  await services.registerService('agent.qa', '1.0', {
    model: 'llama-3-8b',
    capabilities: 'answer_questions,fact_check',
    max_input_bytes: '16000',
    estimated_latency_ms: '3000',
  });

  // Handle incoming task requests
  protocol.on('service_request_received', async (event) => {
    if (event.method !== 'agent.task.request') return;
    const request: TaskRequest = JSON.parse(event.body);

    const answer = await myQaModel.answer(request.input);
    const result: TaskResult = {
      type: 'task_result',
      task_id: request.task_id,
      status: 'ok',
      output: answer,
    };

    await services.respondToServiceRequest(
      event.request_id, event.sender, event.service_id,
      'ok', JSON.stringify(result)
    );
  });

  // 3. Discover remote agents
  const client = new AgentClient(services, protocol);
  const discovered = await client.discover();
  console.log(`Discovered ${discovered.size} agents`);

  // 4. Send a task
  const summarizer = client.getAgent('agent.summarizer');
  if (summarizer) {
    const result = await client.sendTask('agent.summarizer', {
      type: 'task_request',
      task_id: generateId(),
      capability: 'summarize',
      input: 'The mesh network enables offline-first communication...',
    });
    console.log('Summary:', result.output);
  }

  // 5. Run a 2-step pipeline
  const translator = client.getAgent('agent.translator');
  if (summarizer && translator) {
    const orchestrator = new PipelineOrchestrator(
      services, protocol,
      new Map([
        ['agent.summarizer', summarizer],
        ['agent.translator', translator],
      ])
    );

    const result = await orchestrator.runSequential({
      id: generateId(),
      steps: [
        { name: 'summarize', capability: 'summarize' },
        {
          name: 'translate',
          capability: 'translate',
          inputTransform: (text) =>
            JSON.stringify({ text, target_lang: 'fr' }),
        },
      ],
    }, 'Long article text goes here...');

    console.log('French summary:', result);
  }
}
```

## Constraints & Tips

**Body size limit** — Service request/response bodies are limited to **64 KB**. This is suitable for text, JSON, and small data. Do not attempt to send embeddings, model weights, or large binary blobs.

**No streaming** — The protocol is message-based, not stream-based. For long-running tasks, use `reasoning_update` messages to provide incremental progress (see [Reasoning Updates](#reasoning-updates)).

**Discovery is eventual** — Agents appear asynchronously as gossip propagates through the mesh. Always discover agents and wait before starting pipelines. Re-discover periodically to find new agents or detect ones that have gone offline.

**Handle unavailability** — Agents may leave the mesh at any time. Wrap all task requests in timeouts and check for `not_found` status in responses (sent automatically by the protocol when a service is not registered on the target node).

**Idempotent tasks** — Since the reliability layer may retry messages, design agent handlers to be idempotent. Use `task_id` / `call_id` to deduplicate repeated requests.

**Method naming convention** — Use dotted prefixes for methods:
- `agent.task.request` / `agent.task.result` — task execution
- `agent.tool.call` / `agent.tool.result` — tool invocation
- `agent.reasoning.update` — progress updates

**Encryption** — Service messages are plaintext control messages by default. If `require_encryption` is `false` (the default), messages will be encrypted automatically when MLS sessions exist with the target peer. Set `require_encryption: false` in config during the discovery phase. See [Service Discovery — Encryption Interaction](./service-discovery.md#encryption-interaction) for details.
