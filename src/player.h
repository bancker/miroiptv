#ifndef TV_PLAYER_H
#define TV_PLAYER_H

#include "common.h"
#include <libavformat/avformat.h>
#include <libavcodec/avcodec.h>
#include <libswscale/swscale.h>
#include <libswresample/swresample.h>
#include <pthread.h>

typedef struct {
    AVFormatContext  *fmt;
    int               video_idx;
    int               audio_idx;
    AVCodecContext   *vctx;
    AVCodecContext   *actx;
    struct SwsContext *sws;         /* to YUV420P if needed */
    struct SwrContext *swr;         /* to s16 stereo 48k */

    queue_t           video_q;      /* holds video_frame_t* */
    queue_t           audio_q;      /* holds audio_chunk_t* */

    pthread_t         thread;
    volatile int      stop;
    int               audio_sample_rate_out;
} player_t;

int  player_open(player_t *p, const char *url);
void player_close(player_t *p);
int  player_start(player_t *p);            /* spawns decoder thread */
void player_stop(player_t *p);             /* signals stop, joins */

#endif
