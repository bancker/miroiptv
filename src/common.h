#ifndef TV_COMMON_H
#define TV_COMMON_H

#include <stdint.h>
#include <stddef.h>
#include <pthread.h>

/* ---- frame / audio chunk payloads ---- */

/* Raw decoded video frame (YUV420P). The decoder owns malloc, main owns free. */
typedef struct {
    int      width;
    int      height;
    int      stride_y;
    int      stride_u;
    int      stride_v;
    uint8_t *y;     /* width * height */
    uint8_t *u;     /* width/2 * height/2 */
    uint8_t *v;     /* width/2 * height/2 */
    double   pts;   /* seconds, stream time */
} video_frame_t;

/* Decoded audio chunk, s16le stereo. */
typedef struct {
    int16_t *samples;     /* interleaved L/R */
    size_t   n_samples;   /* per channel count */
    int      sample_rate;
    double   pts;         /* seconds, stream time */
} audio_chunk_t;

void video_frame_free(video_frame_t *f);
void audio_chunk_free(audio_chunk_t *c);

/* ---- SPSC bounded queue, void* payload ---- */

typedef struct {
    void           **slots;
    size_t           capacity;
    size_t           head;
    size_t           tail;
    size_t           count;
    int              closed;         /* producer said "no more" */
    pthread_mutex_t  mu;
    pthread_cond_t   not_full;
    pthread_cond_t   not_empty;
} queue_t;

int   queue_init(queue_t *q, size_t capacity);
void  queue_destroy(queue_t *q);                     /* frees remaining items with the callback in free_fn */
void  queue_destroy_with(queue_t *q, void (*free_fn)(void *));
int   queue_push(queue_t *q, void *item);            /* blocks when full; returns -1 if closed */
void *queue_pop(queue_t *q);                         /* blocks when empty; returns NULL when closed & empty */
void  queue_close(queue_t *q);                       /* wakes everyone */
void  queue_drain(queue_t *q, void (*free_fn)(void *));

#endif
