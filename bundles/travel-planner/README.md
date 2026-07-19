# Travel Planner Bundle

> Product class: `Operational Bundle`

Independent, signed `travel.*` Bundle runtime for trip, itinerary, constraint,
budget and export capabilities. It uses only the public Bundle SDK/runtime
support and the Core broker; it owns no database pool or Core-private module.
Manifest v3 declares this package as `Operational`; travel research and
distillation belong to separate Intelligence Bundles.

Build a package with `scripts/build-package.sh`. For local 18085 development,
`scripts/stage-dev-package.sh` signs and atomically publishes it under
`.gadgetron/bundles/travel-planner`.
