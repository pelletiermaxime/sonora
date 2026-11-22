# Fork metal discovery and listening history

nav-discover = Discover

empath-new-releases = New releases

empath-top-genres = Top bands by genre

empath-genres-description = Explore highly rated metal bands by genre.

empath-ranking-help = Bands ranked by empath's adjusted reviewer ratings, with at least 20 reviewers. Scores are not album review percentages. Select a band to find its music.

empath-credit = empath · Updated { $date }

empath-cache-help = Data loads automatically and stays cached for 24 hours.

empath-loading-genres = Loading genre rankings

empath-loading-similar = Loading similar bands

empath-load-failed = Could not load empath data

empath-retry-help = Use Refresh results to try again.

empath-stale = Showing saved empath data because the source could not be updated.

empath-no-filter-match = No bands match your search in this genre.

empath-band-count = { $count ->
    [one] 1 band
   *[other] { $count } bands
    }

empath-rank = #{ $rank }

empath-rating = empath score { $score }

empath-similarity = Similarity { $score }

empath-reviewers = { $count ->
    [one] 1 reviewer
   *[other] { $count } reviewers
    }

empath-shared-reviewers = { $count ->
    [one] 1 shared reviewer
   *[other] { $count } shared reviewers
    }

empath-similar-title = Similar bands · empath

empath-similar-help = Similarity based on shared Metal Archives reviewers, adjusted for popularity and small samples. Select a band to find its music.

empath-no-similar = empath has no similar-band data for this artist in its current snapshot.

empath-choose-identity = Several bands have this name. Choose the Metal Archives identity to load its similar bands.

empath-identity = { $artist } · Archives ID { $id }

empath-show-less = Show fewer bands

empath-show-all = Show all { $count } similar bands

empath-find-title = Find { $artist }

empath-find-help = Choose a matching artist from your active music provider. Bands with the same name may be different artists.

empath-finding-band = Finding this band in your music provider...

empath-find-failed = Could not search your music provider. Close this window and select the band to try again.

empath-no-provider-band = No matching artist was found in your active music provider.

empath-provider-match = { $artist } · { $id }

metal-discover-description = Browse new metal releases by week, with bands you know highlighted by your listening history.

listening-tab = Your listening

listening-description = Your top artists and albums, ranked by all-time plays from your saved Last.fm history.

listening-top-artists = Top artists

listening-lookup-title = Find { $name }

listening-lookup-help = Choose a matching page in your active music provider. Namesakes and album editions may differ.

listening-lookup-failed = Could not search your music provider. Close this window and try the link again.

listening-no-match = No matching page was found in your active music provider.

listening-open-artist = Open artist

listening-top-albums = Top albums

listening-account = Last.fm: { $user }

listening-updated = Last updated: { $date }

listening-artist-count = { $count ->
    [one] 1 artist
   *[other] { $count } artists
    }

listening-album-count = { $count ->
    [one] 1 album
   *[other] { $count } albums
    }

listening-plays = { $count ->
    [one] personal play
   *[other] personal plays
    }

listening-loading = Loading your listening history

listening-cache-help = This tab reads your shared Last.fm cache. Browsing and filtering do not request new data.

listening-cache-failed = Could not read your saved listening history

listening-fetch-failed = Could not update your Last.fm history

listening-fetch-failed-help = Saved rankings remain available. If this history has not been loaded yet, try loading it again shortly.

listening-connect = Connect Last.fm to see your listening history

listening-connect-help = Add your Last.fm account in Settings. Sonora will reuse the history fetched for Discover and artist pages.

listening-empty = No artists or albums to show

listening-missing-help = This history has not been saved yet. Load it once to use the shared cache.

listening-empty-help = Clear your search to see more results. Album totals that are unavailable are left out of the ranking.

listening-load-artists = Load artist history

listening-load-albums = Load album history

metal-discover-cache-policy = Releases load automatically and stay cached for 24 hours. Refresh results to check for updates.

metal-discover-browser-title = Metal Archives — load new releases

metal-discover-browser-progress = Loading your release list. This window will close automatically when it is ready.

metal-discover-loading = Loading new releases

metal-discover-verifying = Complete any verification in the temporary browser window. Your releases will appear here automatically.

metal-discover-failed = Could not update releases

metal-discover-stale = Showing your saved releases while the update is unavailable.

metal-discover-empty-help = Try another week, change the filter, or clear your search.

metal-visible-count = { $count ->
    [one] 1 release
   *[other] { $count } releases
    }

metal-personalized = Familiar bands highlighted using { $user }'s Last.fm history

metal-new-to-you = New to you

metal-new-to-you-help = Full-length albums from Metal Archives bands you have played at most five times, with at least 25,000 Last.fm listeners. Every eligible band is checked automatically.

metal-personalization-progress = { $checked } of { $total ->
    [one] 1 eligible band checked
   *[other] { $total } eligible bands checked
    }

metal-familiar-help = Releases from bands in your Last.fm listening history.

metal-personalizing = Finding albums from bands you rarely listen to...

metal-personalization-failed = Last.fm personalization is temporarily unavailable. You can still browse all releases.

metal-filter-hint = Filter by band, album, genre, or release type

metal-familiar = From bands you know

metal-worth-checking = Worth checking out

metal-all = All releases

metal-full-lengths = Full-length albums

metal-known = Familiar bands

metal-this-week = This week

metal-summary = { $count ->
    [one] 1 release
   *[other] { $count } releases
    } / { $artists ->
    [one] 1 artist in your Last.fm history
   *[other] { $artists } artists in your Last.fm history
    }

metal-history-source = Listening history: { $source }

metal-request-policy = Artist Metal Archives data loads on request; Discover loads releases automatically. Results are cached for 24 hours, with at least two seconds between requests and at most 100 a day. Browser source links are outside this budget. Recommendations use full-length albums, at most 5 personal plays, and at least 25,000 Last.fm listeners. Last.fm data is name-based and may include namesakes.

metal-loading = Loading Metal Archives data...

metal-checking-dates = Checking recent release dates in the background. You can browse the discography now.

metal-album-lookup-failed = Could not find streaming albums. Try again.

metal-no-releases = No releases match this date range and filter.

metal-source = Archives

metal-artist-plays = { $artist } / { $count ->
    [one] 1 personal play
   *[other] { $count } personal plays
    }

metal-personal-plays = { $count ->
    [one] 1 personal play on Last.fm
   *[other] { $count } personal plays on Last.fm
    }

metal-album-personal-plays = { $count ->
    [one] 1 personal album play on Last.fm
   *[other] { $count } personal album plays on Last.fm
    }

metal-album-plays-unavailable = Album plays unavailable on Last.fm

metal-loading-album-plays = Updating your Last.fm album plays in the background...

metal-album-plays-failed = Album play counts are temporarily unavailable. Cached counts remain visible; try again shortly.

metal-album-plays-rate-limited = Last.fm is limiting requests. Try again in { $minutes ->
    [one] 1 minute.
   *[other] { $minutes } minutes.
    }

metal-retry-album-plays = Retry album plays

metal-listeners = { $count ->
    [one] 1 Last.fm listener
   *[other] { $count } Last.fm listeners
    }

metal-review-score = { $score }% / { $count ->
    [one] 1 review
   *[other] { $count } reviews
    }

metal-load = Load data

metal-refresh = Refresh results

metal-load-help = Load Metal Archives data when you want it. Opening this page does not request it. Artist lookups open a temporary browser if the site requires verification.

metal-cache-policy = Successful results are cached for 24 hours. Refresh requests new data; older cached results remain visible if it fails. Artist lookups can open a temporary browser for site verification.

metal-cache-expired = These cached results are older than 24 hours. Refresh when you want updated data.

metal-reader-title = Metal Archives

metal-tab-main = Main albums

metal-tab-singles = Singles & EPs

metal-tab-other = Live, demos & other

metal-tab-all = All

metal-no-tab-releases = No releases in this category. Choose All to see the complete discography.

metal-score-percent = { $score }%

metal-review-count = { $count ->
    [one] 1 review
   *[other] { $count } reviews
    }

metal-not-reviewed = Not reviewed

metal-upcoming = Upcoming

metal-release-on = Releases { $date }

metal-date-unconfirmed = Release date unconfirmed

metal-date-check-help = Refresh results to check

metal-browser-title = Metal Archives — verify and select a band

metal-browser-unavailable = Sonora could not open its native browser. On Linux, install WebKitGTK 4.1 or 4.0 and its runtime dependencies.

metal-browser-closed = The browser was closed before the data was ready. Refresh the results to try again.

metal-browser-wrong-artist = The selected band has a different artist name. Load or refresh the data and choose the correct band.

metal-browser-incomplete = The browser did not return complete Metal Archives data. Refresh the results to try again.

metal-browser-timed-out = The browser did not return data within five minutes. Refresh the results to try again.

metal-discography = Metal Archives discography

metal-open-archives = Open Metal Archives

metal-access-forbidden = Metal Archives refused this lookup (HTTP 403). Refresh the results to try again.

metal-access-rate-limited = Metal Archives limited this lookup (HTTP 429).

metal-access-challenge = Metal Archives requires browser verification. Complete it in the temporary browser window.

metal-access-retry-after = Metal Archives asked Sonora to wait before another request.

metal-access-daily-budget = Try again in { $hours ->
    [one] 1 hour
   *[other] { $hours } hours
    }.

metal-access-until = Try again after { $time }. Cached results remain available.

metal-fetch-failed = Could not load Metal Archives data. Load or refresh the data to try again.

metal-no-discography = No discography was returned for this band.

metal-no-band = No matching band was found on Metal Archives.

metal-select-band = Choose the correct band, genre and country before loading a discography.

metal-change-band = Check band identity

metal-match-title = Find { $album }

metal-match-help = Results from your active music provider. Check the album title and artist ({ $artist }) before playing; editions and namesakes may differ.

metal-matching = Searching your music provider...

metal-no-album = No matching album was found on your active music provider.

metal-open-album = Open

metal-sign-in = Sign in to a streaming provider to find and play this release.

metal-partial = Cached data contains { $count } of { $total } releases. Refresh to load the complete listing.

library-play-liked-songs = Play
