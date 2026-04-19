#ifndef TV_PLAYER_H
#define TV_PLAYER_H

#include "common.h"
#include <libavformat/avformat.h>
#include <libavcodec/avcodec.h>
#include <libswscale/swscale.h>
#include <libswresample/swresample.h>
#include <pthread.h>

#define PLAYER_MAX_AUDIO_TRACKS 8

typedef struct {
    AVFormatContext  *fmt;
    int               video_idx;
    int               audio_idx;   /* current AV stream index (one of audio_tracks[]) */
    AVCodecContext   *vctx;
    AVCodecContext   *actx;
    struct SwsContext *sws;         /* to YUV420P if needed */
    struct SwrContext *swr;         /* to s16 stereo 48k */

    queue_t           video_q;      /* holds video_frame_t* */
    queue_t           audio_q;      /* holds audio_chunk_t* */

    pthread_t         thread;
    volatile int      stop;
    int               audio_sample_rate_out;
    long long         video_frames_pushed;
    long long         audio_frames_pushed;

    /* Multi-track audio. Dubbed VOD files commonly ship with 2-3 audio PIDs
     * (FR, NL, EN), and the default pick is "first stream found" — which is
     * how "No Time to Die (NL)" ended up playing the French dub track.
     *
     * The decoder thread watches audio_track_req; when it differs from
     * audio_track_cur it flushes the current audio codec context, reopens
     * it for the new stream, and drains audio_q so the user doesn't hear
     * 2s of the old language tailing off. swr_context gets rebuilt too
     * because the new stream can have a different sample-rate or channel
     * layout. */
    int               audio_tracks[PLAYER_MAX_AUDIO_TRACKS];  /* AV stream indices */
    char              audio_lang[PLAYER_MAX_AUDIO_TRACKS][16]; /* "nld"/"fra"/... or "" */
    int               n_audio_tracks;
    int               audio_track_cur;  /* index into audio_tracks[] the decoder is ON */
    volatile int      audio_track_req;  /* index into audio_tracks[] the UI wants */
} player_t;

int  player_open(player_t *p, const char *url);
void player_close(player_t *p);
int  player_start(player_t *p);            /* spawns decoder thread */
void player_stop(player_t *p);             /* signals stop, joins */

/* Request the decoder thread switch to audio_tracks[track_idx]. track_idx
 * is 0..n_audio_tracks-1. Returns 0 on a valid request, -1 out of range.
 * The switch is asynchronous; audio audibly changes after the current
 * audio queue drains (~300ms typical). */
int  player_set_audio_track(player_t *p, int track_idx);

#endif
