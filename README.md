# navidrome

Self-hosted music server plus the pipeline that turns a Spotify playlist export
into playlists over a local library. Runs on k3s on `pc1`.

```
Exportify zip  ->  /opt/navidrome/import  ->  CronJob (importer)  ->  PLAYLIST tags + .m3u
                                                                          |
                                                                     Navidrome
                                                                          |
ListenBrainz weekly  ->  Explo (yt-dlp)  ->  /opt/navidrome/music/explo  ->  Subsonic playlist
```

## Explo

[Explo](https://github.com/LumePart/Explo) pulls the ListenBrainz Weekly
Exploration playlist, downloads the tracks it does not already have, drops them
under the music library, and creates the matching playlist over Navidrome's
Subsonic API. Web UI on port 7288.

### Credentials

`41-explo-secret.yaml` is a SealedSecret: encrypted against the sealed-secrets
controller on `pc1`, so only that cluster can read it and the tree stays safe to
publish. To rotate a credential, write a plaintext env file (`*.env` is
gitignored) and re-seal:

```
kubectl create secret generic explo-secrets \
  --namespace music --from-env-file=./explo.env \
  --dry-run=client -o yaml \
  | kubeseal --format yaml > k8s/41-explo-secret.yaml
```

Six keys: `LISTENBRAINZ_USER`, `LISTENBRAINZ_USER_TOKEN`, `SYSTEM_USERNAME`,
`SYSTEM_PASSWORD`, `UI_USERNAME`, `UI_PASSWORD`. The first two come from
listenbrainz.org, the next two are a Navidrome login, the last two are the login
for Explo's own web UI.

`SYSTEM_USERNAME` has to be a Navidrome admin: Explo calls `startScan` after a
download so tracks appear without waiting for `ND_SCANSCHEDULE`.

The sealing key lives only on `pc1`. Back it up, or a rebuilt node cannot
decrypt anything in this repo:

```
kubectl get secret -n kube-system \
  -l sealedsecrets.bitnami.com/sealed-secrets-key -o yaml > sealed-secrets-key.yaml
```

Non-secret settings are in the `explo-config` ConfigMap. Values set there
override whatever the web UI wizard writes to the `.env` on the config PVC, so
treat the ConfigMap as the source of truth and use the UI for inspection.

Downloads come from YouTube via `yt-dlp`. Whether that is legal depends on the
track and on where you are.
