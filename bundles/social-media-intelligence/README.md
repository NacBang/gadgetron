# Social Media Intelligence

> Product class: `Intelligence Bundle`

Independent Intelligence Bundle for followed public conversations, signals, briefings, and
reviewable response drafts. The runtime owns `social.*` domain records and has no network access.
Core owns provider requests, rate limits, purgeable Source blobs, Review, Graph projection, Penny,
and every cross-Bundle knowledge handoff.

The first provider is the official Bluesky public AppView search API. Version 1 does not read
private messages or automate follows, likes, replies, or publishing. Response assistance produces
revision-pinned drafts with `publish_allowed = false`.
