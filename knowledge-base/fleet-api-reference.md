# Fleet Controller API Reference (v3)

Base URL: `https://api.quokka-robotics.example/v3`. All requests require a bearer token from the customer admin console.

## Rate limits

| Plan       | Requests per minute | Burst |
|------------|---------------------|-------|
| Starter    | 120                 | 20    |
| Pro        | 1,200               | 200   |
| Enterprise | 6,000               | 1,000 |

Exceeding the limit returns HTTP 429 with a `Retry-After` header in seconds.

## Endpoints

- `GET /robots`: list robots with status (`idle`, `working`, `charging`, `parked`, `fault`).
- `GET /robots/{id}`: one robot, including battery level and current task.
- `POST /tasks`: create a task. Body: `{"type": "pick" | "move_pallet" | "scan", "robot_id": optional, "priority": 1-5}`. If `robot_id` is omitted the scheduler assigns one.
- `DELETE /tasks/{id}`: cancel a task that has not started. Running tasks cannot be cancelled through the API; use `POST /robots/{id}/stop`.
- `POST /robots/{id}/stop`: emergency stop. The robot halts within 300 ms and needs a manual reset on site.
- `GET /fleet/health`: aggregated fleet health and the current SLA month's measured uptime.

## Error codes

| Code | Meaning |
|------|---------|
| 400  | Invalid request body |
| 401  | Missing or expired token |
| 403  | Token lacks the required scope (`fleet:read`, `fleet:write`, `fleet:stop`) |
| 409  | Robot is busy or in fault state |
| 429  | Rate limit exceeded |
| 503  | Fleet controller degraded; retry with exponential backoff |

## Webhooks

Register a webhook to receive `robot.fault`, `task.completed` and `fleet.degraded` events. Webhooks are retried for up to 24 hours with exponential backoff and signed with an HMAC-SHA256 signature in the `X-Quokka-Signature` header.
