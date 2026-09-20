# TypeSafe Jev collection (Lapis)

Post-retrieve **query↔chunk** gate. Transport is Facet TypeSafe / System One —
same secret hydration as Facet, no second key in Lattice. Contract:
[Jev native](../../jev-native.md).

| | |
| --- | --- |
| Environment | `typesafe` — hydrate `typesafeApiKey` via Facet |
| Shadow | `jevShadow=true` — classify only; empty / uncertain ≠ approve |

```bash
# Hydrate once (Facet machine store). Never put the key in notes or sqlite.
facet env set docs/examples/typesafe --environment typesafe \
  --name typesafeApiKey --value "$TYPESAFE_API_KEY" --secret

lapis search welcome --rerank-jev
```
