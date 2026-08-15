//! Per-connection OAuth state: the current token, and the flow that renews it.
//!
//! One session per server URL. It holds the bearer token every outgoing request
//! reads, and serializes authorization behind a single mutex so that concurrent
//! `401`s run one flow rather than several. [`FlowState`] is what a completed
//! flow leaves behind -- the client and server metadata a later refresh needs in
//! order to run without the user.
//!
//! Two things here are subtler than they look: which [`TokenStore`] slot a
//! credential belongs to (`store_key_for`, which is not always the configured
//! issuer), and the SEP-2350 rule that a step-up asks for the scopes the session
//! already had *plus* the challenged one.

use super::*;

/// The OAuth client and authorization-server metadata retained from the
/// last successful flow -- everything a non-interactive token refresh
/// needs.
pub(super) struct FlowState {
    pub(super) client: OAuthClient,
    pub(super) metadata: AuthorizationServerMetadata,
    /// The [`TokenStore`] slot this flow's credentials went into -- the
    /// identity they actually belong to, which is not always the one the
    /// configuration names. See [`OAuthSession::store_key_for`].
    pub(super) store_key: Arc<str>,
}

/// How early before expiration a stored access token is proactively
/// refreshed. Mirrors the leeway `OAuthClient::token` applies, so the
/// cheap staleness probe and the actual refresh decision agree.
const REFRESH_LEEWAY: std::time::Duration = std::time::Duration::from_secs(30);

/// Per-connection OAuth state: the current access token and the
/// single-flight authorization flow.
pub(crate) struct OAuthSession {
    pub(super) config: OAuthClientConfig,
    /// Canonicalized server URL -- the RFC 8707 resource indicator and the
    /// discovery base.
    pub(super) resource: String,
    /// Where this session's credentials live in the [`TokenStore`]: the
    /// resource, prefixed by the issuer they came from when one is configured.
    ///
    /// It starts out naming the *configured* issuer, because it is read before
    /// any discovery has happened, and moves to the issuer a flow actually ran
    /// against -- which is the slot that flow filed its tokens in. The two
    /// differ only where a portable identity is allowed to outlive a migration
    /// ([`OAuthSession::store_key_for`]); left at the configured one, the
    /// staleness probe would keep looking into an empty slot and every renewal
    /// would have to wait for a `401` to notice.
    pub(super) store_key: RwLock<Arc<str>>,
    /// Current bearer token, read on every outgoing request.
    pub(super) token: RwLock<Option<Arc<str>>>,
    /// Serializes authorization flows (concurrent 401s run one flow) and
    /// caches the client + metadata for non-interactive refresh.
    pub(super) flow: Mutex<Option<FlowState>>,
    /// Scopes the last completed flow asked for.
    ///
    /// A re-authorization asks for these *plus* whatever the new challenge
    /// demands (SEP-2350): a token minted for the challenged scope alone would
    /// lose access the session already had, and the next call for the old scope
    /// would challenge straight back.
    pub(super) requested_scopes: RwLock<Vec<String>>,
}

impl std::fmt::Debug for OAuthSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthSession")
            .field("resource", &self.resource)
            .finish()
    }
}

impl OAuthSession {
    /// Builds a session for the MCP server at `server_url`.
    pub(crate) fn new(config: OAuthClientConfig, server_url: &str) -> Result<Self, Error> {
        // Before anything else, so a configuration that cannot produce a
        // working flow is reported where it was written rather than at the
        // first `401` -- which may be a long-running process away.
        config.validate()?;

        let resource = canonicalize_resource_uri(server_url)
            .map_err(|err| Error::new(ErrorCode::InternalError, err.to_string()))?;

        let store_key = Self::initial_store_key(&config, &resource);

        let token = config
            .store
            .get(&store_key)
            .filter(|tokens| !tokens.is_expired())
            .map(|tokens| tokens.access_token.into());

        Ok(Self {
            config,
            resource,
            store_key: RwLock::new(store_key.into()),
            token: RwLock::new(token),
            flow: Mutex::new(None),
            requested_scopes: RwLock::new(Vec::new()),
        })
    }

    /// The key this session reads before it has discovered anything.
    pub(super) fn store_key(&self) -> Arc<str> {
        self.store_key
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Moves the session onto the slot the last flow actually used, so the
    /// pre-discovery reads that follow -- the staleness probe, the stored
    /// grant -- look where that flow wrote rather than where the
    /// configuration guessed.
    pub(super) fn set_store_key(&self, key: &str) {
        let mut current = self
            .store_key
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        if **current != *key {
            *current = Arc::from(key);
        }
    }

    /// Where this session's credentials live in the [`TokenStore`].
    ///
    /// The key names the whole identity a credential belongs to -- which
    /// authorization server issued it, which client it was issued to, and
    /// which resource it is for -- as `{issuer}|{client}|{resource}`. Any part
    /// the configuration does not name is left empty, and a credential whose
    /// identity is not fully named is never reused
    /// ([`Self::may_reuse_stored_refresh`] holds to exactly that).
    ///
    /// **Issuer.** The spec's own prescription: credentials are to be
    /// associated with the authorization server that issued them, "keyed by
    /// the authorization server's `issuer` identifier". Keyed by the resource
    /// alone, nothing records where a stored credential came from, so after a
    /// migration the *current* configuration is all a check has to go on -- and
    /// the current configuration is exactly what an operator updates when the
    /// resource moves. The old server's refresh token would be read back under
    /// the same key and offered to the new one.
    ///
    /// **Client.** Two clients are two grants: the user consented to each
    /// separately, for scopes they chose separately. Sharing a slot has the
    /// second client send the first one's access token -- consent it was never
    /// given -- and, worse, present the first one's refresh token under its own
    /// id, which an authorization server reads as a stolen token and answers by
    /// revoking the grant for both. Naming the client keeps the two apart. A
    /// pre-registered id and a document URL are both stable across restarts, so
    /// either serves; a dynamically registered client has none until the flow
    /// it is about to run mints one, and leaves this empty.
    ///
    /// A segment is a configured value, so a `|` written into one blurs the
    /// boundary with the next. It takes an operator putting one there, and the
    /// values that come from elsewhere -- a document URL, the resource -- are
    /// validated URIs, where `|` is not a legal character.
    ///
    /// This one is built from the *configured* issuer, because it is read
    /// before any discovery has happened. Once a flow knows which server it is
    /// actually talking to, [`Self::store_key_for`] is what files what that
    /// server minted.
    pub(super) fn initial_store_key(config: &OAuthClientConfig, resource: &str) -> String {
        Self::compose_store_key(
            config.issuer.as_deref().unwrap_or_default(),
            config.client_identity(),
            resource,
        )
    }

    /// Where credentials minted by `issuer` belong -- the key every read and
    /// write from inside a flow uses, once discovery has named the server.
    ///
    /// It is the discovered issuer rather than the configured one because the
    /// key is a statement about where these tokens *came from*, and the two
    /// part company exactly where a portable identity is allowed to: a Client
    /// ID Metadata Document resolves at whichever server meets it, so a CIMD
    /// client whose resource has moved completes its flow against a server the
    /// configuration does not name. Filing that server's tokens under the
    /// configured issuer would mislabel them -- and if the resource ever moved
    /// back, the configured key would hand the *old* server a refresh token
    /// the *new* one minted, which is the leak the keying exists to stop.
    ///
    /// An unbound session names no issuer here either: it has nothing to say
    /// about where its tokens came from, so the segment stays empty and the
    /// slot stays the one its own warm start reads.
    ///
    /// The client segment comes from `source` rather than from the
    /// configuration, because the two part company when a configured document
    /// meets a server that does not resolve one: the flow falls back to
    /// registration, and what it obtains belongs to that throwaway client, not
    /// to the document. Filing it under the document would have a later flow
    /// -- against a server that has since enabled documents, say -- read it
    /// back as the document's own and present it under a client id that never
    /// held it, which the server answers with `invalid_grant` and this client
    /// answers by discarding the entry and asking the user again.
    pub(super) fn store_key_for(&self, issuer: &str, source: ClientIdSource<'_>) -> String {
        let issuer = match self.config.issuer {
            Some(_) => issuer,
            None => "",
        };
        Self::compose_store_key(issuer, source.persistent_id(), &self.resource)
    }

    /// Joins the three parts of a credential's identity into its store key.
    pub(super) fn compose_store_key(issuer: &str, client: &str, resource: &str) -> String {
        format!("{issuer}|{client}|{resource}")
    }

    /// Scopes this session is known to hold, most authoritative source first.
    ///
    /// The in-memory set records what the last flow *in this process* was
    /// granted, so it is empty after a restart -- and a persistent
    /// [`TokenStore`] hands back a token whose grant the process never saw. Left
    /// at that, the first `insufficient_scope` challenge after a restart would
    /// build its step-up from the demanded scopes alone and trade away
    /// everything the restored token already carried, which is the opposite of
    /// what SEP-2350 asks for. So a stored token's own `scope` -- what RFC 6749
    /// has the authorization server report as *granted* -- stands in for it.
    ///
    /// A server may omit `scope` when it granted exactly what was asked
    /// (RFC 6749 section 5.1), leaving nothing recorded. Configured scopes
    /// answer that case: they are what every flow of this session requests, so
    /// they are held by construction.
    pub(super) fn requested_scopes(&self) -> Vec<String> {
        let asked = self
            .requested_scopes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if !asked.is_empty() {
            return asked;
        }

        self.config
            .store
            .get(&self.store_key())
            .and_then(|tokens| tokens.scope)
            .map(|granted| split_scopes(&granted))
            .filter(|granted| !granted.is_empty())
            .or_else(|| self.config.scopes.clone())
            .unwrap_or_default()
    }

    pub(super) fn set_requested_scopes(&self, scopes: Vec<String>) {
        *self
            .requested_scopes
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = scopes;
    }

    /// The current bearer token, if any.
    pub(crate) fn bearer(&self) -> Option<Arc<str>> {
        self.token
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(super) fn set_token(&self, token: Arc<str>) {
        *self
            .token
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(token);
    }

    /// The bearer token to attach to the next request, proactively
    /// refreshed when the stored set is about to expire and a refresh
    /// token is available -- the session then renews without user
    /// interaction. Falls back to the current token when refresh is not
    /// possible; the `401` path handles the rest.
    pub(crate) async fn refreshed_bearer(&self) -> Option<Arc<str>> {
        // Cheap staleness probe before taking the flow lock.
        let stale = self
            .config
            .store
            .get(&self.store_key())
            .is_some_and(|tokens| tokens.expires_within(REFRESH_LEEWAY));

        if !stale {
            return self.bearer();
        }

        let mut flow = self.flow.lock().await;
        self.maintain(&mut flow).await.or_else(|| self.bearer())
    }

    /// Non-interactive token maintenance through the cached client:
    /// serves the stored set, refreshing it when stale (rotation
    /// carry-over and dead-entry pruning included, via
    /// `OAuthClient::token`). Returns `None` when interactive
    /// authorization is required or no flow has completed yet.
    pub(super) async fn maintain(&self, state: &mut Option<FlowState>) -> Option<Arc<str>> {
        let FlowState {
            client,
            metadata,
            store_key,
        } = state.as_ref()?;
        self.refresh_with(client, metadata, store_key.clone()).await
    }

    /// [`Self::maintain`] for a client and metadata held directly rather than
    /// cached -- what the reconstruct-after-restart path has in hand.
    pub(super) async fn refresh_with(
        &self,
        client: &OAuthClient,
        metadata: &AuthorizationServerMetadata,
        store_key: Arc<str>,
    ) -> Option<Arc<str>> {
        // What the grant was known to cover going in. A refresh response may
        // leave `scope` out when the grant is unchanged (RFC 6749 section 5.1),
        // and the renewed set *replaces* the stored one -- so a renewal would
        // otherwise erase the only record of what the token carries. The next
        // `insufficient_scope` challenge would then widen from nothing and
        // trade the grant away, which is the very thing SEP-2350 forbids. The
        // refresh token itself is carried over for the same reason one step
        // down, inside `OAuthClient::token`.
        let carried = self
            .config
            .store
            .get(&store_key)
            .and_then(|tokens| tokens.scope);

        match client.token(&store_key, metadata).await {
            Ok(Some(mut tokens)) => {
                // What the renewed token covers: what the response said it
                // granted, or -- when it said nothing -- the grant it did not
                // restate.
                let granted = tokens.scope.clone().or(carried);
                if tokens.scope.is_none()
                    && let Some(scope) = granted.clone()
                {
                    tokens.scope = Some(scope);
                    self.config.store.put(&store_key, &tokens);
                }
                // And the in-memory record moves with it. A refresh may
                // *narrow* the grant, and this process's memory of the earlier,
                // wider one outranks the store -- so a challenge demanding
                // something the renewed token no longer carries would read as
                // already covered, take the single-flight shortcut, and hand
                // back that same token to be refused again on the request's one
                // retry.
                if let Some(scope) = granted.as_deref() {
                    let scopes = split_scopes(scope);
                    if !scopes.is_empty() {
                        self.set_requested_scopes(scopes);
                    }
                }
                // This slot is where the session's credentials live now, which
                // matters when the key moved: a portable identity may have
                // renewed against a server the configuration does not name.
                self.set_store_key(&store_key);
                let token: Arc<str> = tokens.access_token.into();
                self.set_token(token.clone());
                Some(token)
            }
            // Nothing renewable -- interactive authorization it is.
            Ok(None) => None,
            // Transient failure (issuer unreachable): keep the current
            // token and let the request outcome decide.
            Err(_err) => {
                #[cfg(feature = "tracing")]
                tracing::warn!(logger = "neva", "token refresh failed: {_err}");
                None
            }
        }
    }

    /// Runs the authorization flow triggered by a `401` and returns the
    /// fresh bearer token.
    ///
    /// `www_authenticate` is the challenge header value, when present --
    /// its `resource_metadata` pointer takes precedence over well-known
    /// derivation. `used` is the token the failed request carried:
    /// concurrent callers that lost the race simply pick up the token
    /// the winning flow produced.
    pub(crate) async fn authorize(
        &self,
        www_authenticate: Option<&str>,
        used: Option<&str>,
    ) -> Result<Arc<str>, Error> {
        let mut flight = self.flow.lock().await;

        let challenge = www_authenticate.and_then(|header| BearerChallenge::parse(header).ok());
        // Scopes the challenge demands that this session has never asked for.
        // A refresh cannot widen a grant, so their presence is what separates
        // "this token expired" from "this token is not enough" -- the second
        // needs the user back, however fresh the token is.
        let demanded = challenge
            .as_ref()
            .and_then(|challenge| challenge.scope())
            .map(split_scopes)
            .unwrap_or_default();

        // `insufficient_scope` is itself the statement that this grant is too
        // narrow, and RFC 6750 leaves the `scope` attribute optional -- so a
        // server may say it without naming what it wants. Reading only the
        // named scopes would call that "not a step-up", take the refresh path,
        // and spend the exchange's one retry on a token that is short by
        // exactly as much as before.
        let insufficient = challenge.as_ref().is_some_and(|challenge| {
            matches!(
                challenge.error(),
                Some(volga_oauth_client::OAuthErrorCode::InsufficientScope)
            )
        });

        // Read after the lock, so a flow that finished while this caller queued
        // behind it is already accounted for.
        let held = self.requested_scopes();
        let uncovered = demanded.iter().any(|scope| !held.contains(scope));

        let step_up = insufficient || uncovered;

        // A step-up that named no scope leaves nothing to check coverage
        // against, and a token that merely changed proves nothing: a refresh
        // rotates the access token without touching what it covers, and any
        // other request in this process may have run one while this caller
        // queued. Taking it would be the refresh path under another name --
        // exactly what reading `insufficient_scope` was meant to stop -- and the
        // exchange's one retry would go out just as short as before.
        let unverifiable = step_up && demanded.is_empty();

        // Someone else may have completed a widening flow while this caller
        // waited on the lock, and its token is right here. Taking it is the
        // whole point of the single-flight lock: two callers refused for the
        // same missing scope must not walk the user through consent twice.
        //
        // Trustworthy only because both halves are checked: the grant on record
        // now covers what the challenge demanded, *and* the token is not the one
        // that was just refused.
        if !uncovered
            && !unverifiable
            && let Some(current) = self.bearer()
            && used != Some(&*current)
        {
            return Ok(current);
        }

        // A configured set is the caller's decision about what this client may
        // ever ask for, and the flow below honors it to the letter -- so a
        // challenge naming something outside it describes a grant this client
        // cannot obtain. Running the flow anyway is the worst of both: it
        // interrupts the user for consent and still comes back without the one
        // scope the call needed, so the retry is refused exactly as before.
        // Widening past the configured set is no answer either -- it would
        // override the decision, and an authorization server refuses a scope
        // the client is not registered for. So this ends here, naming the
        // scope, because adding it to `with_scopes` is the only thing that
        // resolves it.
        if step_up && let Some(configured) = &self.config.scopes {
            let missing = demanded
                .iter()
                .filter(|scope| !configured.contains(scope))
                .cloned()
                .collect::<Vec<_>>();
            if !missing.is_empty() {
                return Err(Error::new(
                    ErrorCode::InvalidRequest,
                    format!(
                        "the server requires scope `{}`, which this client is not \
                         configured to request; add it to `with_scopes`",
                        missing.join(" ")
                    ),
                ));
            }
        }

        // Refresh before interrupting the user: a stored refresh token
        // renews the session silently. A token identical to the rejected
        // one is no help though (revoked server-side) -- interactive then.
        if !step_up
            && let Some(token) = self.maintain(&mut flight).await
            && used != Some(&*token)
        {
            return Ok(token);
        }

        let stated = challenge
            .as_ref()
            .and_then(|challenge| challenge.resource_metadata().map(str::to_owned));

        let discovery = DiscoveryClient::with_config(self.config.client_config());
        let resource_metadata = match stated {
            // The challenge named the document: that is the answer, and a
            // failure there is the failure -- guessing elsewhere would be
            // discovering a document the server did not point at.
            Some(url) => discovery
                .fetch_resource_metadata_from_url(&url, Some(&self.resource))
                .await
                .map_err(flow_error)?,
            None => self.discover_resource_metadata(&discovery).await?,
        };

        let server_metadata = discovery
            .discover_authorization_server(&resource_metadata)
            .await
            .map_err(flow_error)?;

        let source = self.config.client_id_source(&server_metadata);
        self.check_issuer_binding(source, &server_metadata)?;

        // Nothing configured, nowhere to register, and the server does not
        // resolve document URLs: there is no way for this client to obtain an
        // id here, and the spec's last resort -- ask a human for one -- means
        // saying so. Said now, before a redirect listener is bound and long
        // before a browser opens on a flow that ends in `invalid_client`.
        if source == ClientIdSource::Dynamic && server_metadata.registration_endpoint.is_none() {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                format!(
                    "`{}` supports neither dynamic client registration nor client id \
                     metadata documents, so this client cannot obtain a client id there; \
                     register one out of band and configure it with `with_client_id`",
                    server_metadata.issuer
                ),
            ));
        }

        let redirect_uri = self.config.handler.redirect_uri().await?;
        let client = self
            .build_client(source, &server_metadata, &redirect_uri)
            .await?;

        // A durable [`TokenStore`] outlives the process; the flow state that
        // knows how to use it does not. So after a restart the refresh attempt
        // above found nothing to refresh *with* -- no client, no metadata --
        // and a stored refresh token, still perfectly good, went unused while
        // the user was walked through consent again. Both halves have just been
        // rebuilt, so ask once more before that.
        // The slot this flow's credentials belong in, named once and used for
        // every read and write below.
        let store_key: Arc<str> =
            Arc::from(self.store_key_for(&server_metadata.issuer, source).as_str());

        if !step_up
            && self.may_reuse_stored_refresh(source, &server_metadata)
            && let Some(token) = self
                .refresh_with(&client, &server_metadata, store_key.clone())
                .await
            && used != Some(&*token)
        {
            // Keep what made it work, so the next refresh is the cheap path.
            *flight = Some(FlowState {
                client,
                metadata: server_metadata,
                store_key,
            });
            return Ok(token);
        }

        // What to ask for, most specific first. A configured set is the
        // caller's decision and overrides everything -- and by here it already
        // covers whatever the challenge demanded, since a demand outside it
        // ended this call above. Otherwise the challenge names what this very
        // request needed, which is narrower and more current than the
        // resource's advertised set; `scopes_supported` is the fallback, and an
        // empty one means asking for no `scope` at all.
        let mut scopes = match &self.config.scopes {
            Some(configured) => configured.clone(),
            None if !demanded.is_empty() => demanded.clone(),
            None => resource_metadata.scopes_supported.clone(),
        };
        // SEP-2350: carry everything earlier rounds asked for, so a step-up
        // widens the grant instead of trading one scope for another.
        for held in self.requested_scopes() {
            if !scopes.contains(&held) {
                scopes.push(held);
            }
        }

        // The RFC 8707 resource indicator is the identifier the *accepted*
        // metadata declares, not the endpoint this client happens to talk to.
        // They are the same thing whenever the document was found under the
        // endpoint's own path -- that is what validating it checks -- but a
        // document served at the origin describes the origin, and asking for a
        // token audienced to the endpoint would either be refused by an
        // authorization server that enforces its own advertised identifier, or
        // grant a token for an audience the resource never claimed.
        let request = client
            .authorization_request(&server_metadata)
            .with_scopes(scopes.clone())
            .with_resource(resource_metadata.resource.clone())
            .build()
            .map_err(flow_error)?;

        let params = self.config.handler.authorize(request.url.clone()).await?;

        if !request.matches_state(&params.state) {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "authorization response `state` mismatch",
            ));
        }
        validate_issuer(&params, &server_metadata)?;

        let tokens = client
            .exchange_code(&server_metadata, &params.code, &request)
            .await
            .map_err(flow_error)?;

        // What the server *granted*, which is not always what was asked for.
        // RFC 6749 section 5.1 has the token response state `scope` whenever it
        // differs from the request and omit it when it matches, so the response
        // is the authority and the request is only the fallback. Recording the
        // request would count a scope that was asked for and refused as held --
        // and then the challenge that names it reads as "this token expired"
        // rather than "this grant is too narrow", so the client refreshes into
        // the same refusal instead of widening.
        let mut tokens = tokens;
        let granted = tokens
            .scope
            .as_deref()
            .map(split_scopes)
            .filter(|granted| !granted.is_empty())
            .unwrap_or(scopes);

        // A grant inferred from the request goes into the stored set too, not
        // just into memory. The store is what outlives the process, and the
        // omission that produced this inference -- "granted exactly what you
        // asked for" -- is the common case, so leaving it unwritten would have
        // the next run start out believing it holds nothing and let the first
        // step-up replace the grant instead of widening it.
        if tokens.scope.is_none() && !granted.is_empty() {
            tokens.scope = Some(granted.join(" "));
        }

        self.config.store.put(&store_key, &tokens);
        self.set_store_key(&store_key);

        // Keep the client + metadata so future refreshes stay
        // non-interactive.
        *flight = Some(FlowState {
            client,
            metadata: server_metadata,
            store_key,
        });
        self.set_requested_scopes(granted);

        let token: Arc<str> = tokens.access_token.into();
        self.set_token(token.clone());
        Ok(token)
    }

    /// Refuses credentials that belong to a different authorization server
    /// than the one the resource now points at.
    ///
    /// A `client_id` obtained out of band is issued *by* one authorization
    /// server; it identifies nothing at another. So when the resource's
    /// metadata starts naming a different issuer, presenting it there is at
    /// best an `invalid_client` refusal and at worst hands an attacker-run
    /// server a credential and the user's consent for it. The spec has the
    /// client surface an error instead, and only the client knows which
    /// server its configured credentials came from -- hence
    /// [`with_issuer`](OAuthClientConfig::with_issuer).
    ///
    /// A Client ID Metadata Document URL is deliberately exempt: it is
    /// resolved by whichever server meets it, so it is portable across them
    /// by design and a change of issuer asks nothing of it. Dynamic
    /// registration is exempt for the opposite reason -- the id is minted
    /// against this very server, moments from now.
    pub(super) fn check_issuer_binding(
        &self,
        source: ClientIdSource<'_>,
        metadata: &AuthorizationServerMetadata,
    ) -> Result<(), Error> {
        let ClientIdSource::PreRegistered(client_id) = source else {
            return Ok(());
        };
        let Some(bound_to) = &self.config.issuer else {
            return Ok(());
        };
        if bound_to == &metadata.issuer {
            return Ok(());
        }

        Err(Error::new(
            ErrorCode::InvalidRequest,
            format!(
                "client `{client_id}` is registered with `{bound_to}`, but \
                 `{}` now names `{}` as its authorization server; \
                 credentials are not portable between them",
                self.resource, metadata.issuer
            ),
        ))
    }

    /// Whether the refresh token sitting in the store may be offered to
    /// `metadata`'s token endpoint.
    ///
    /// Two things have to hold, and neither is implied by the other. The
    /// client id must be the one the token was issued to, which rules out
    /// dynamic registration: the id this flow is about to mint is not the one
    /// from last time, and a refresh token belongs to the client it was
    /// issued to. And the token has to have come from *this* authorization
    /// server -- a refresh token is a bearer credential for the endpoint that
    /// minted it, so sending it to a server that did not is handing that
    /// server a credential for another one.
    ///
    /// The second is settled by [`Self::store_key_for`] rather than here: the
    /// read goes to the slot `metadata`'s issuer files its own tokens in, so
    /// whatever comes back came from the server it is about to be sent to. All
    /// that is left to check is whether this session's slots carry an issuer
    /// at all. Unbound they do not -- there is one unlabelled slot, and a
    /// credential out of it proves nothing about where it came from, so the
    /// session re-authorizes interactively instead: a worse experience and the
    /// only safe answer.
    ///
    /// Deliberately *not* a comparison against the configured issuer. That
    /// would refuse a portable identity whose resource has migrated -- a CIMD
    /// client is allowed to complete a flow against a server its configuration
    /// does not name, and refusing to renew what that flow obtained would walk
    /// the user through consent on every restart. The slot it renews from is
    /// labelled with that same server, which is the assurance the comparison
    /// was standing in for.
    pub(super) fn may_reuse_stored_refresh(
        &self,
        source: ClientIdSource<'_>,
        metadata: &AuthorizationServerMetadata,
    ) -> bool {
        if !source.survives_a_restart() {
            return false;
        }

        if self.config.issuer.is_none() {
            #[cfg(feature = "tracing")]
            tracing::debug!(
                logger = "neva",
                "not offering the stored refresh token to {}: the credentials name no \
                 issuer, so nothing says it came from there. Set `with_issuer` to reuse it.",
                metadata.issuer
            );
            return false;
        }

        true
    }

    /// Finds the Protected Resource Metadata for a server that issued a `401`
    /// without saying where it lives.
    ///
    /// RFC 9728 puts the document under the resource's own path
    /// (`/.well-known/oauth-protected-resource/mcp` for a server at `/mcp`), so
    /// that is asked first. A server that hosts one MCP endpoint often serves it
    /// at the root instead, which is a location the path-based derivation never
    /// reaches -- so a `404` falls back there rather than failing the flow over
    /// a document that exists.
    ///
    /// Strictly a `404`, and not "the first attempt did not work out". Any
    /// other failure means the path-based location answered, and what it said
    /// stands: falling back past a malformed body or a mismatched `resource`
    /// would trade an authoritative refusal for a document describing something
    /// else.
    pub(super) async fn discover_resource_metadata(
        &self,
        discovery: &DiscoveryClient,
    ) -> Result<volga_oauth_client::ProtectedResourceMetadata, Error> {
        let path_based = protected_resource_metadata_url(&self.resource)
            .map_err(|err| Error::new(ErrorCode::InternalError, err.to_string()))?;

        let first = discovery
            .fetch_resource_metadata_from_url(&path_based, Some(&self.resource))
            .await;

        let Err(err) = first else {
            return first.map_err(flow_error);
        };

        // Only "there is no document here" opens the fallback. Every other
        // failure is the path-based document *answering*, and its answer is the
        // authoritative one: a body that does not parse, a `resource` that
        // names something else, a rejected plain-HTTP URL, a TLS or connection
        // failure. Treating those as absence would let a document that failed
        // validation be replaced by one from the origin, which is how a client
        // ends up authorizing against metadata for a different resource than
        // the one it just refused.
        if !matches!(err, ClientError::Http(status) if status.as_u16() == 404) {
            return Err(flow_error(err));
        }

        let Some(origin) = origin_of(&self.resource) else {
            return Err(flow_error(err));
        };

        let root = format!("{origin}{WELL_KNOWN_PROTECTED_RESOURCE}");
        if root == path_based {
            return Err(flow_error(err));
        }

        #[cfg(feature = "tracing")]
        tracing::debug!(
            logger = "neva",
            "no resource metadata at {path_based}; trying {root}"
        );

        // Checked against the origin, not against the endpoint. A document at
        // the root describes the whole origin as the protected resource -- that
        // is what puts it there rather than under the endpoint's path -- so
        // demanding it name the endpoint would reject every document this
        // fallback exists to find. The binding it does keep is the one that
        // matters: the document has to name the origin it was served from.
        discovery
            .fetch_resource_metadata_from_url(&root, Some(&origin))
            .await
            // Both attempts are named: reporting only one of them leaves the
            // reader guessing which location was the problem.
            .map_err(|root_err| {
                Error::new(
                    ErrorCode::InternalError,
                    format!(
                        "OAuth flow failed: no usable resource metadata \
                         at {path_based} ({err}) or {root} ({root_err})"
                    ),
                )
            })
    }

    /// Builds the [`OAuthClient`] for the identity `source` names: a
    /// configured id, a Client ID Metadata Document URL, or one obtained
    /// through dynamic registration (RFC 7591).
    pub(super) async fn build_client(
        &self,
        source: ClientIdSource<'_>,
        server_metadata: &AuthorizationServerMetadata,
        redirect_uri: &str,
    ) -> Result<OAuthClient, Error> {
        let client = match source {
            ClientIdSource::PreRegistered(client_id) => {
                let mut client = OAuthClient::new(client_id);
                if let Some(secret) = &self.config.client_secret {
                    client = client.with_secret(secret.clone());
                }
                client
            }
            // Nothing to register: the URL *is* the id, and the server
            // resolves it to the document the deployer published. A CIMD
            // client is public by construction, so no secret is attached even
            // if one were configured -- and `OAuthClientConfig::validate`
            // refuses that pairing before it can get this far.
            ClientIdSource::Document(url) => OAuthClient::new(url),
            ClientIdSource::Dynamic => {
                let registration = RegistrationClient::with_config(self.config.client_config());
                let response = registration
                    .register(server_metadata, &registration_metadata(redirect_uri))
                    .await
                    .map_err(flow_error)?;
                OAuthClient::from_registration(&response).map_err(flow_error)?
            }
        };

        Ok(client
            .with_config(self.config.client_config())
            .with_redirect_uri(redirect_uri)
            .with_token_store(self.config.store.clone()))
    }
}
