---
name: planista
description: Publish complete HTML documents to a Planista server and return short public permalinks. Use when an agent needs to share an HTML plan, report, prototype, or other self-contained page in a GitHub issue, pull request, comment, or chat.
---

# Planista

Publish HTML only when the user intends it to be public. Treat every permalink as unlisted, not private, and never include credentials, tokens, or confidential data.

## Publish a document

1. Obtain the Planista origin from the user or the `PLANISTA_URL` environment variable. Do not guess a server URL.
2. Produce a complete HTML document. Keep it under the server's default 10 MiB limit unless the deployment documents another limit.
3. Post the raw document and capture the returned permalink:

```bash
permalink=$(curl --fail-with-body --silent --show-error \
  -H 'Content-Type: text/html; charset=utf-8' \
  --data-binary @plan.html \
  "${PLANISTA_URL%/}/")
printf '%s\n' "$permalink"
```

4. Put the permalink in the destination requested by the user.

Planista returns `413` when the document is too large and `507` when the server has reached its plan limit. Report either condition directly instead of retrying.

Do not search for, expose, or invoke Planista's rotating administrator wipe URL.
