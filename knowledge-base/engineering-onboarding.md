# Engineering Onboarding Guide

Welcome to Quokka Robotics engineering. This guide covers your first two weeks.

## Week 1

- Day 1: laptop pickup at the IT desk (Ghent office, 2nd floor), SSO setup, security training.
- Day 2: architecture overview session with your onboarding buddy. Read the QFC design docs in the wiki.
- Day 3-5: ship a small change. Every new engineer merges at least one pull request in week 1, usually a documentation or test fix.

## Week 2

- Shadow an on-call shift (you are not paged yet).
- Attend the weekly fleet review on Thursday at 14:00 CET.
- Complete the robot safety certification (2-hour course, required before touching physical robots in the test lab).

## Tooling

- Code: GitHub, trunk-based development, every PR needs one review and green CI.
- Languages: the fleet controller is written in Rust; robot firmware in C++ and Rust; data tooling in Python.
- Deploys: QFC deploys to production every weekday at 11:00 CET via the release train. Hotfixes require an incident commander's approval.
- Observability: metrics and traces in Grafana; logs in Loki; alerts route through the central alerting system (since April 2026).

## On-call

Engineers join the on-call rotation after 8 weeks. Rotations are one week long, Monday 09:00 to Monday 09:00 CET. On-call compensation is EUR 350 per week plus EUR 60 per hour for incidents handled outside business hours.
