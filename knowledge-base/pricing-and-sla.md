# Quokka Fleet Controller Plans and Service Level Agreement

Every customer runs their robots through the Quokka Fleet Controller (QFC). QFC is sold in three plans.

## Plans

| Plan       | Price per robot per month | Monthly uptime commitment | Support response time (P1) |
|------------|---------------------------|---------------------------|----------------------------|
| Starter    | EUR 49                    | 99.5%                     | 8 business hours           |
| Pro        | EUR 99                    | 99.9%                     | 1 hour, 24/7               |
| Enterprise | EUR 179                   | 99.95%                    | 15 minutes, 24/7, named TAM |

RaaS customers get the Pro plan included in their monthly fee. They can upgrade to Enterprise for EUR 80 per robot per month extra.

## How uptime is measured

Uptime is measured per calendar month as the percentage of minutes in which the QFC API and the robot command channel were both available. Planned maintenance announced at least 72 hours in advance does not count as downtime. A month is counted as 30 days (43,200 minutes) for SLA purposes, regardless of its actual length.

## Service credits

If uptime falls below the plan's commitment, the customer is entitled to a service credit on that month's QFC fee:

| Monthly uptime achieved           | Credit (Starter) | Credit (Pro) | Credit (Enterprise) |
|-----------------------------------|------------------|--------------|---------------------|
| Below commitment but at least 99.0% | 5%             | 10%          | 15%                 |
| 98.0% to below 99.0%              | 10%              | 20%          | 30%                 |
| 95.0% to below 98.0%              | 15%              | 30%          | 50%                 |
| Below 95.0%                       | 25%              | 50%          | 100%                |

Credits are capped at 100% of the monthly QFC fee and must be claimed by the customer within 30 days after the end of the affected month. Credits are applied to the next invoice and are never paid out in cash. Credits apply to the QFC fee only; for RaaS customers the QFC share of the monthly fee is fixed at EUR 99 per robot.
