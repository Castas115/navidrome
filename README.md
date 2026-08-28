# navidrome

Self-hosted music server plus the pipeline that turns a Spotify playlist export
into playlists over a local library. Runs on k3s on `pc1`.

```
Exportify zip  ->  /opt/navidrome/import  ->  CronJob (importer)  ->  PLAYLIST tags + .m3u
                                                                          |
                                                                     Navidrome
```

