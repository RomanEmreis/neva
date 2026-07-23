# OAuth with Keycloak

The full MCP authorization flow against a real OAuth 2.1/OIDC issuer:
a neva server validating tokens against Keycloak's JWKS, and a neva
client walking discovery -> browser login -> PKCE code exchange -> tool
calls, including a role-gated tool.

## 1. Start Keycloak

From the repository root:

```bash
docker run --rm -p 8080:8080 \
  -e KC_BOOTSTRAP_ADMIN_USERNAME=admin \
  -e KC_BOOTSTRAP_ADMIN_PASSWORD=admin \
  -v $(pwd)/examples/oauth-with-keycloak/realm-export.json:/opt/keycloak/data/import/neva-realm.json \
  quay.io/keycloak/keycloak:26.0 start-dev --import-realm
```

The imported `neva` realm contains:

| Item | Value | Purpose |
|---|---|---|
| public client | `neva-mcp-client`, redirect `http://127.0.0.1:8919/callback`, PKCE S256 | the MCP client |
| audience mapper | adds `http://127.0.0.1:3000/mcp` to `aud` | RFC 8707 binding the server verifies |
| role mapper | realm roles -> flat `roles` claim | what neva's `DefaultClaims` / `#[tool(roles = [...])]` read |
| user | `demo` / `demo`, realm role `admin` | the person in the browser |

## 2. Start the MCP server

```bash
cargo run -p example-oauth-with-keycloak --bin keycloak-server
```

One `with_oauth(|oauth| oauth.with_issuer(...))` call does everything:
JWKS-based token validation, `aud`/`iss` checks, and the RFC 9728
metadata document on
`http://127.0.0.1:3000/.well-known/oauth-protected-resource/mcp`.

## 3. Run the client

```bash
cargo run -p example-oauth-with-keycloak --bin keycloak-client
```

The first request hits a `401`, the client discovers Keycloak through
the challenge, and the system browser opens the login page -- sign in as
`demo` / `demo`. After the redirect the client retries transparently and
calls both tools; `admin_report` works because `demo` carries the
`admin` realm role. Tokens are refreshed in the background afterwards.

## MCP Inspector

The server also works with the Inspector's guided OAuth flow:

```bash
npx @modelcontextprotocol/inspector
```

Connect with transport `Streamable HTTP` to `http://127.0.0.1:3000/mcp`.
In the authentication settings set **Client ID** to `neva-mcp-client`
(Keycloak restricts anonymous dynamic client registration by default,
so the Inspector cannot register itself), then follow the prompts and
sign in as `demo` / `demo`.

The Inspector's **redirect URL field is read-only by design** -- the
callback must land on the Inspector web app itself
(`http://localhost:6274/oauth/callback`, plus `.../callback/debug` for the
guided flow). Both are pre-registered on `neva-mcp-client` by the realm
import; if you run the Inspector on a non-default port, add the matching
URIs in the Keycloak admin console (Clients -> `neva-mcp-client` -> Valid
redirect URIs).

## Notes

* Local Keycloak runs over plain `http`, so both binaries relax
  `require_https` for discovery/token traffic. Never do that outside
  local development.
* The client is pre-registered because Keycloak restricts anonymous
  dynamic client registration by default; against an issuer with open
  DCR the `with_client_id`/`with_port` configuration disappears and the
  client registers itself (as `application_type: "native"`).
