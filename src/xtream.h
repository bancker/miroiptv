#ifndef TV_XTREAM_H
#define TV_XTREAM_H

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

#endif
