/* tests/test_queue.c — 15 tests for queue_t from src/queue.c */
#include "../src/common.h"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <pthread.h>

#ifdef _WIN32
#  include <windows.h>
#endif

/* ---- tests ---- */

static void test_queue_init_destroy(void) {
    queue_t q;
    assert(queue_init(&q, 8) == 0);
    queue_destroy(&q);
    puts("OK test_queue_init_destroy");
}

static void test_queue_push_pop_single(void) {
    queue_t q;
    assert(queue_init(&q, 8) == 0);
    int val = 42;
    assert(queue_push(&q, &val) == 0);
    int *out = queue_pop(&q);
    assert(out == &val);
    assert(*out == 42);
    queue_destroy(&q);
    puts("OK test_queue_push_pop_single");
}

static void test_queue_push_pop_many(void) {
    queue_t q;
    assert(queue_init(&q, 8) == 0);
    int vals[8];
    for (int i = 0; i < 8; ++i) {
        vals[i] = i * 10;
        assert(queue_push(&q, &vals[i]) == 0);
    }
    for (int i = 0; i < 8; ++i) {
        int *p = queue_pop(&q);
        assert(p == &vals[i]);
        assert(*p == i * 10);
    }
    queue_destroy(&q);
    puts("OK test_queue_push_pop_many");
}

static void test_queue_wraps_around(void) {
    queue_t q;
    assert(queue_init(&q, 8) == 0);
    int vals[12];
    for (int i = 0; i < 12; ++i) vals[i] = i;

    /* Push 8 */
    for (int i = 0; i < 8; ++i) assert(queue_push(&q, &vals[i]) == 0);
    /* Pop 4 — head advances to 4 */
    for (int i = 0; i < 4; ++i) {
        int *p = queue_pop(&q);
        assert(p == &vals[i]);
    }
    /* Push 4 more — tail wraps around */
    for (int i = 8; i < 12; ++i) assert(queue_push(&q, &vals[i]) == 0);
    /* Pop remaining 8 in FIFO order */
    for (int i = 4; i < 12; ++i) {
        int *p = queue_pop(&q);
        assert(p == &vals[i]);
        assert(*p == i);
    }
    queue_destroy(&q);
    puts("OK test_queue_wraps_around");
}

static void test_queue_try_pop_empty_returns_null(void) {
    queue_t q;
    assert(queue_init(&q, 8) == 0);
    void *p = queue_try_pop(&q);
    assert(p == NULL);
    queue_destroy(&q);
    puts("OK test_queue_try_pop_empty_returns_null");
}

/* Consumer thread: blocks on queue_pop; stores result in arg. */
typedef struct { queue_t *q; void *result; } pop_arg_t;

static void *consumer_pop(void *arg) {
    pop_arg_t *a = arg;
    a->result = queue_pop(a->q);
    return NULL;
}

static void test_queue_close_signals_pop(void) {
    queue_t q;
    assert(queue_init(&q, 8) == 0);
    pop_arg_t arg = { &q, (void *)0x1 };

    pthread_t thr;
    assert(pthread_create(&thr, NULL, consumer_pop, &arg) == 0);
    /* Give the consumer time to block. */
#ifdef _WIN32
    Sleep(30);
#else
    {struct timespec ts = {0, 30*1000*1000}; nanosleep(&ts, NULL);}
#endif
    queue_close(&q);
    pthread_join(thr, NULL);
    /* queue_pop returns NULL when queue is closed and empty. */
    assert(arg.result == NULL);
    queue_destroy(&q);
    puts("OK test_queue_close_signals_pop");
}

static void test_queue_close_rejects_push(void) {
    queue_t q;
    assert(queue_init(&q, 8) == 0);
    queue_close(&q);
    int val = 99;
    assert(queue_push(&q, &val) == -1);
    queue_destroy(&q);
    puts("OK test_queue_close_rejects_push");
}

static void test_queue_drain_empties(void) {
    queue_t q;
    assert(queue_init(&q, 8) == 0);
    int vals[5];
    for (int i = 0; i < 5; ++i) {
        vals[i] = i;
        assert(queue_push(&q, &vals[i]) == 0);
    }
    queue_drain(&q, NULL);
    assert(q.count == 0);
    queue_destroy(&q);
    puts("OK test_queue_drain_empties");
}

static void test_queue_drain_calls_free_fn(void) {
    queue_t q;
    assert(queue_init(&q, 8) == 0);
    for (int i = 0; i < 5; ++i) {
        int *p = calloc(1, sizeof(int));
        assert(p);
        *p = i;
        assert(queue_push(&q, p) == 0);
    }
    queue_drain(&q, free);  /* free each malloc'd int */
    assert(q.count == 0);
    queue_destroy(&q);
    puts("OK test_queue_drain_calls_free_fn");
}

/* Producer thread: pushes one item then stores push result. */
typedef struct {
    queue_t *q;
    void    *item;
    int      result;
} push_arg_t;

static void *producer_push(void *arg) {
    push_arg_t *a = arg;
    a->result = queue_push(a->q, a->item);
    return NULL;
}

static void test_queue_drain_wakes_producer(void) {
    queue_t q;
    assert(queue_init(&q, 2) == 0);

    /* Fill queue to capacity. */
    int v1 = 1, v2 = 2, v3 = 3;
    assert(queue_push(&q, &v1) == 0);
    assert(queue_push(&q, &v2) == 0);

    /* Start producer that will block because queue is full. */
    push_arg_t parg = { &q, &v3, -99 };
    pthread_t thr;
    assert(pthread_create(&thr, NULL, producer_push, &parg) == 0);

    /* Give the producer a moment to block. */
#ifdef _WIN32
    Sleep(30);
#else
    {struct timespec ts = {0, 30*1000*1000}; nanosleep(&ts, NULL);}
#endif

    /* Drain should wake the blocked producer. */
    queue_drain(&q, NULL);

    pthread_join(thr, NULL);
    /* Producer should have pushed successfully after drain freed space. */
    assert(parg.result == 0);
    queue_drain(&q, NULL);
    queue_destroy(&q);
    puts("OK test_queue_drain_wakes_producer");
}

/* SPSC stress test. */
#define SPSC_N 10000

typedef struct { queue_t *q; int *buf; int n; } spsc_arg_t;

static void *spsc_producer(void *arg) {
    spsc_arg_t *a = arg;
    for (int i = 0; i < a->n; ++i) queue_push(a->q, &a->buf[i]);
    queue_close(a->q);
    return NULL;
}

static void test_queue_spsc_stress(void) {
    queue_t q;
    assert(queue_init(&q, 64) == 0);

    int *items = malloc(SPSC_N * sizeof(int));
    assert(items);
    for (int i = 0; i < SPSC_N; ++i) items[i] = i;

    spsc_arg_t parg = { &q, items, SPSC_N };
    pthread_t thr;
    assert(pthread_create(&thr, NULL, spsc_producer, &parg) == 0);

    int received = 0;
    int expected = 0;
    while (1) {
        int *p = queue_pop(&q);
        if (!p) break;
        assert(*p == expected);
        expected++;
        received++;
    }

    pthread_join(thr, NULL);
    assert(received == SPSC_N);
    free(items);
    queue_destroy(&q);
    puts("OK test_queue_spsc_stress");
}

static void test_queue_capacity_one(void) {
    queue_t q;
    assert(queue_init(&q, 1) == 0);
    int a = 1, b = 2;
    assert(queue_push(&q, &a) == 0);
    int *p = queue_try_pop(&q);
    assert(p == &a);
    assert(queue_push(&q, &b) == 0);
    p = queue_try_pop(&q);
    assert(p == &b);
    assert(queue_try_pop(&q) == NULL);
    queue_destroy(&q);
    puts("OK test_queue_capacity_one");
}

static void test_queue_try_pop_after_push(void) {
    queue_t q;
    assert(queue_init(&q, 8) == 0);
    int val = 77;
    assert(queue_push(&q, &val) == 0);
    int *p = queue_try_pop(&q);
    assert(p == &val);
    assert(*p == 77);
    assert(queue_try_pop(&q) == NULL);
    queue_destroy(&q);
    puts("OK test_queue_try_pop_after_push");
}

static void test_queue_destroy_zeroed_is_safe(void) {
    queue_t q = {0};
    queue_destroy(&q);   /* must not crash — guard in queue.c:37 */
    puts("OK test_queue_destroy_zeroed_is_safe");
}

static void test_queue_push_nonblocking_when_not_full(void) {
    /* Single-threaded: push 3 items into cap=8 queue with no consumer.
     * None of the pushes should block because the queue is not full. */
    queue_t q;
    assert(queue_init(&q, 8) == 0);
    int vals[3] = {10, 20, 30};
    for (int i = 0; i < 3; ++i) assert(queue_push(&q, &vals[i]) == 0);
    assert(q.count == 3);
    /* Pop them back out to verify correctness. */
    for (int i = 0; i < 3; ++i) {
        int *p = queue_try_pop(&q);
        assert(p && *p == vals[i]);
    }
    queue_destroy(&q);
    puts("OK test_queue_push_nonblocking_when_not_full");
}

/* ---- main ---- */

int main(void) {
    test_queue_init_destroy();
    test_queue_push_pop_single();
    test_queue_push_pop_many();
    test_queue_wraps_around();
    test_queue_try_pop_empty_returns_null();
    test_queue_close_signals_pop();
    test_queue_close_rejects_push();
    test_queue_drain_empties();
    test_queue_drain_calls_free_fn();
    test_queue_drain_wakes_producer();
    test_queue_spsc_stress();
    test_queue_capacity_one();
    test_queue_try_pop_after_push();
    test_queue_destroy_zeroed_is_safe();
    test_queue_push_nonblocking_when_not_full();
    return 0;
}
