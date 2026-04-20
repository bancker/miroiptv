#include "../src/favorites.h"
#include <assert.h>
#include <stdio.h>

int main(void) {
    favorites_t fv = {0};
    favorites_init(&fv, NULL);
    favorites_free(&fv);
    puts("OK stub_links");
    return 0;
}
