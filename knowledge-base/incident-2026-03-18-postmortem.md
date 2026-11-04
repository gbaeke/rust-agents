# Postmortem: Fleet Controller outage, 18 March 2026

Status: final. Severity: P1. Author: Site Reliability team.

## Summary

On 18 March 2026 the Quokka Fleet Controller (QFC) in the eu-west region was unavailable for 4 hours and 12 minutes, from 06:41 to 10:53 CET. Robots in affected fleets finished their current task and then parked safely, as designed. No safety incidents occurred.

This was unplanned downtime and was not announced in advance. There was no other downtime in eu-west during March 2026.

## Customer impact

All customers whose fleets are homed in eu-west were affected. The largest affected customers were:

| Customer               | Plan       | Robots in fleet |
|------------------------|------------|-----------------|
| Bluefin Logistics      | Enterprise | 140 (QR-1)      |
| Moerbei Foods          | Pro        | 32 (QR-1), 6 (QR-3) |
| Tarnwick Distribution  | RaaS (Pro included) | 18 (QR-1) |
| Sablewood Pharma       | Starter    | 4 (QR-5)        |

Bluefin Logistics reported an estimated 2,300 delayed orders.

## Root cause

The TLS certificate used by the robot command channel's internal load balancer expired at 06:41. Certificate renewal had been automated, but the renewal job had been silently failing since a secrets-store migration on 2 February 2026 because it still referenced the old secrets path. The alert for failing renewals routed to a decommissioned pager channel.

## Timeline (CET)

- 06:41 Certificate expires; robots lose the command channel and park.
- 06:58 First customer ticket (Moerbei Foods).
- 07:20 On-call engineer paged via customer escalation, not via monitoring.
- 08:35 Root cause identified.
- 09:50 New certificate issued manually; rollout blocked by a cached chain on load balancer nodes.
- 10:53 All load balancer nodes restarted; service fully restored.

## Action items

1. Move certificate expiry monitoring to the central alerting system with a 21-day warning (owner: SRE, due 2026-04-15, done).
2. Audit all alert routes for decommissioned channels (owner: SRE, due 2026-04-30, done).
3. Add a synthetic check on the robot command channel every 60 seconds (owner: Platform, due 2026-05-31, done).
4. Proactively notify affected customers about their SLA credit eligibility (owner: Customer Success, due 2026-04-10).
