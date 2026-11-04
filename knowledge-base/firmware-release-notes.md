# Robot Firmware Release Notes

## 4.2.0 (2026-09-02)

- Wombat (QR-1): picking speed improved by 11% through better arm trajectory planning.
- Numbat (QR-3): adds support for half-pallets (600 x 800 mm).
- All robots: battery health reporting via the `GET /robots/{id}` endpoint.
- Known issue: Platypus (QR-5) RFID range drops to 4 m when the thermal camera is active. Workaround: disable thermal during inventory scans. Fix planned for 4.2.1.

## 4.1.3 (2026-06-17)

- Fixes a rare case where Wombat robots parked in charging lanes after a command-channel loss, blocking other robots. Found during the 18 March 2026 outage.
- Improves reconnect behaviour: robots now retry the command channel every 10 seconds instead of every 60 seconds.

## 4.1.0 (2026-03-03)

- New "safe park" mode: on loss of the command channel robots finish their current task and park in designated zones.
- Platypus (QR-5): automatic return-to-dock threshold raised from 15% to 20% battery.

## Support policy

The two most recent minor versions are supported. As of September 2026 that is 4.2.x and 4.1.x. Customers on 4.0.x or older must upgrade to receive support for P2-P4 issues; P1 issues are always supported.
