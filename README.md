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

Its credentials cannot live in `k8s/` — the tree is public and synced by Argo
CD. Create the Secret out of band once:

```
LISTENBRAINZ_USER=... LISTENBRAINZ_USER_TOKEN=... \
SYSTEM_USERNAME=... SYSTEM_PASSWORD=... \
UI_USERNAME=... UI_PASSWORD=... \
  scripts/explo-secret
```

`SYSTEM_USERNAME` has to be a Navidrome admin: Explo calls `startScan` after a
download so tracks appear without waiting for `ND_SCANSCHEDULE`.

Non-secret settings are in the `explo-config` ConfigMap. Values set there
override whatever the web UI wizard writes to the `.env` on the config PVC, so
treat the ConfigMap as the source of truth and use the UI for inspection.

Downloads come from YouTube via `yt-dlp`. Whether that is legal depends on the
track and on where you are.
