#ifndef TV_XTREAM_H
#define TV_XTREAM_H

#include "npo.h"  /* epg_t, epg_entry_t reused from the NPO module */

/* Xtream Codes portal credentials. host/user/pass are malloc'd strings owned
 * by the struct; port is an integer (portal's HTTP port, typically 8080). */
typedef struct {
    char *host;
    int   port;
    char *user;
    char *pass;
} xtream_t;

/* Parses "user:pass@host[:port]" (port defaults to 8080). Returns 0 on
 * success and populates *out; caller must xtream_free it. Returns -1 on
 * malformed input. */
int  xtream_parse(const char *spec, xtream_t *out);
void xtream_free(xtream_t *x);

/* Builds an HLS manifest URL for the given Xtream Codes stream id.
 * Returns a malloc'd string; caller frees. */
char *xtream_stream_url(const xtream_t *x, int stream_id);

/* Per-channel stream IDs for this portal (m.hnlol.com). HD variants chosen
 * for sensible-quality default. Index matches NPO_CHANNELS[] in npo.h. */
extern const int XTREAM_NPO_STREAM_IDS[3];

/* Catch-up ("TERUGKIJKEN") stream IDs — these have tv_archive=1 and support
 * the /timeshift/... URL pattern. Separate from live IDs because the portal
 * provisions catch-up on a distinct channel number. 2-day window. */
extern const int XTREAM_NPO_ARCHIVE_STREAM_IDS[3];

/* RTL channel identifiers come in two flavours on this portal:
 *  - LIVE stream IDs (category "NL | NEDERLAND") serve /live/USER/PASS/ID.ts
 *    and are used when we want CURRENT playback. Archive IDs return 502 on
 *    /live/ so they can't substitute.
 *  - ARCHIVE stream IDs (category "NL | TERUGKIJKEN", tv_archive=1) support
 *    the /timeshift/.../START/ID.m3u8 endpoint and 2-day catch-up.
 * Used together by the 'r' hotkey: archive IDs for EPG scan + timeshift
 * URL on past hits, live IDs for airing-now / future hits. */
extern const int XTREAM_RTL_LIVE_STREAM_IDS[5];
extern const int XTREAM_RTL_ARCHIVE_STREAM_IDS[5];
extern const char * const XTREAM_RTL_CHANNEL_NAMES[5];

/* Builds an Xtream Codes timeshift URL for a past event:
 *   http://HOST:PORT/timeshift/USER/PASS/DURATION_MIN/YYYY-MM-DD:HH-MM/ID.ts
 * start_time is a unix timestamp; the URL uses UTC wall-clock format. */
char *xtream_timeshift_url(const xtream_t *x, int archive_stream_id,
                           time_t start_time, int duration_min);

/* Fetches the short EPG (upcoming programs) for a given stream id via the
 * portal's player_api.php?action=get_short_epg endpoint. Titles arrive
 * base64-encoded; we decode them and flag NOS Journaal entries the same way
 * the NPO parser does. Returns 0 on success, -1 on failure. Caller frees
 * with npo_epg_free (same struct).
 *
 * NB: "short" is literal — this only returns ~4 upcoming entries. For past
 * or historical data use xtream_fetch_epg_full below. */
int xtream_fetch_epg(const xtream_t *x, int stream_id, epg_t *out);

/* Full EPG via player_api.php?action=get_simple_data_table. Returns many
 * more entries (observed: 800+ over a multi-day window including past),
 * suitable for searching historical programs like "latest NOS Journaal".
 * Same output shape as xtream_fetch_epg. */
int xtream_fetch_epg_full(const xtream_t *x, int stream_id, epg_t *out);

/* One entry in the portal's live-stream catalog. */
typedef struct {
    int   stream_id;
    int   num;         /* portal's display order */
    char *name;        /* malloc'd, free via xtream_live_list_free */
} xtream_live_entry_t;

typedef struct {
    xtream_live_entry_t *entries;
    size_t               count;
} xtream_live_list_t;

/* Fetches the full live-stream catalog (all channels the account has access
 * to). Typically ~15000 entries. category_id == NULL means "all categories".
 * Returns 0 on success. */
int  xtream_fetch_live_list(const xtream_t *x, const char *category_id,
                            xtream_live_list_t *out);
void xtream_live_list_free(xtream_live_list_t *list);

/* One entry in the portal's VOD catalog (individual movies / specials /
 * standalone releases). Each has its own stream_id and a container
 * extension (mp4/mkv/avi) that must be included in the playback URL. */
typedef struct {
    int   stream_id;
    int   num;
    char *name;           /* malloc'd, free via xtream_vod_list_free */
    char *extension;      /* "mp4" / "mkv" / etc — NULL means portal
                           * omitted it and caller should assume "mp4". */
} xtream_vod_entry_t;

typedef struct {
    xtream_vod_entry_t *entries;
    size_t              count;
} xtream_vod_list_t;

int  xtream_fetch_vod_list(const xtream_t *x, const char *category_id,
                           xtream_vod_list_t *out);
void xtream_vod_list_free(xtream_vod_list_t *list);

/* One series (the "season + episodes" aggregate has its own dedicated
 * endpoint — this list just enumerates series by name). */
typedef struct {
    int   series_id;
    int   num;
    char *name;           /* malloc'd */
} xtream_series_entry_t;

typedef struct {
    xtream_series_entry_t *entries;
    size_t                 count;
} xtream_series_list_t;

int  xtream_fetch_series_list(const xtream_t *x, const char *category_id,
                              xtream_series_list_t *out);
void xtream_series_list_free(xtream_series_list_t *list);

/* One episode of a series — flattened across all seasons. season_num and
 * episode_num let the UI build the "S1E03 - Title" label. */
typedef struct {
    int   id;             /* episode stream-id for /series/... URL */
    int   season_num;
    int   episode_num;
    char *title;          /* malloc'd */
    char *extension;      /* malloc'd, see xtream_vod_entry_t */
} xtream_episode_entry_t;

typedef struct {
    xtream_episode_entry_t *entries;
    size_t                  count;
} xtream_episodes_t;

/* Fetches all episodes for a series, flattened across seasons. Response
 * from player_api.php?action=get_series_info has one object key per
 * season ("1", "2", ...) each holding an array of episode objects. */
int  xtream_fetch_series_info(const xtream_t *x, int series_id,
                              xtream_episodes_t *out);
void xtream_episodes_free(xtream_episodes_t *list);

/* URL builders for VOD movies and series episodes. extension may be NULL —
 * we fall back to "mp4" which the portal accepts for most catalogs. */
char *xtream_vod_url(const xtream_t *x, int stream_id, const char *extension);
char *xtream_series_episode_url(const xtream_t *x, int episode_id, const char *extension);

#endif
