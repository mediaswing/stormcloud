# Security

## Reporting a problem

Please don't report security problems in a public issue. Use GitHub's
[private vulnerability reporting](https://github.com/mediaswing/stormcloud/security/advisories/new)
instead, from the **Security** tab of the repository. Only the maintainer can
see a private report.

A good report says:

- which version of the app you used, and on which operating system
- what the problem is, and what someone could do by taking advantage of it
- the steps or the file that show the problem happening

Please don't include tokens, real tenant IDs or log files from a production
tenant. If a log helps, reproduce the problem in a test tenant first.

Please make sure what you report really happens before you send it.

Problems in Microsoft Entra ID, Microsoft Graph or Exchange Online themselves
should go to the
[Microsoft Security Response Center](https://msrc.microsoft.com/report)
instead.

## What to expect

This is a personal project that I maintain in my spare time, so I can't promise
a reply by a particular date. I'll treat security reports before other work,
and once a problem is fixed I'll publish an advisory that credits you, unless
you'd rather not be named.

## Supported versions

Only the latest release gets security fixes.
