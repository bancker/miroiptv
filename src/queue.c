#include "common.h"
#include <stdlib.h>
#include <string.h>

void video_frame_free(video_frame_t *f) {
    if (!f) return;
    free(f->y); free(f->u); free(f->v);
    free(f);
}

void audio_chunk_free(audio_chunk_t *c) {
    if (!c) return;
    free(c->samples);
    free(c);
}

int queue_init(queue_t *q, size_t capacity) {
    memset(q, 0, sizeof(*q));
    q->slots = calloc(capacity, sizeof(void *));
    if (!q->slots) return -1;
    q->capacity = capacity;
    if (pthread_mutex_init(&q->mu, NULL) != 0) return -1;
    if (pthread_cond_init(&q->not_full, NULL) != 0) return -1;
    if (pthread_cond_init(&q->not_empty, NULL) != 0) return -1;
    return 0;
}

void queue_destroy(queue_t *q) {
    queue_destroy_with(q, NULL);
}

void queue_destroy_with(queue_t *q, void (*free_fn)(void *)) {
    /* Guard against destroying a never-initialized queue — main's error paths
     * may call player_close on a half-open player_t, where only SOME of the
     * queues were init'd. A zeroed queue has slots==NULL and no pthread state
     * to tear down, so skip the teardown in that case. */
    if (q->slots) {
        if (free_fn) queue_drain(q, free_fn);
        free(q->slots);
        pthread_mutex_destroy(&q->mu);
        pthread_cond_destroy(&q->not_full);
        pthread_cond_destroy(&q->not_empty);
    }
    memset(q, 0, sizeof(*q));
}

int queue_push(queue_t *q, void *item) {
    pthread_mutex_lock(&q->mu);
    while (q->count == q->capacity && !q->closed)
        pthread_cond_wait(&q->not_full, &q->mu);
    if (q->closed) { pthread_mutex_unlock(&q->mu); return -1; }
    q->slots[q->tail] = item;
    q->tail = (q->tail + 1) % q->capacity;
    q->count++;
    pthread_cond_signal(&q->not_empty);
    pthread_mutex_unlock(&q->mu);
    return 0;
}

void *queue_pop(queue_t *q) {
    pthread_mutex_lock(&q->mu);
    while (q->count == 0 && !q->closed)
        pthread_cond_wait(&q->not_empty, &q->mu);
    if (q->count == 0) { pthread_mutex_unlock(&q->mu); return NULL; }
    void *item = q->slots[q->head];
    q->head = (q->head + 1) % q->capacity;
    q->count--;
    pthread_cond_signal(&q->not_full);
    pthread_mutex_unlock(&q->mu);
    return item;
}

void *queue_try_pop(queue_t *q) {
    pthread_mutex_lock(&q->mu);
    if (q->count == 0) { pthread_mutex_unlock(&q->mu); return NULL; }
    void *item = q->slots[q->head];
    q->head = (q->head + 1) % q->capacity;
    q->count--;
    pthread_cond_signal(&q->not_full);
    pthread_mutex_unlock(&q->mu);
    return item;
}

void queue_close(queue_t *q) {
    pthread_mutex_lock(&q->mu);
    q->closed = 1;
    pthread_cond_broadcast(&q->not_empty);
    pthread_cond_broadcast(&q->not_full);
    pthread_mutex_unlock(&q->mu);
}

void queue_drain(queue_t *q, void (*free_fn)(void *)) {
    pthread_mutex_lock(&q->mu);
    while (q->count) {
        void *item = q->slots[q->head];
        q->head = (q->head + 1) % q->capacity;
        q->count--;
        if (free_fn) free_fn(item);
    }
    pthread_mutex_unlock(&q->mu);
}
