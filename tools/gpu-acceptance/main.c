// SPDX-License-Identifier: MIT
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/sysctl.h>

extern int nmb_acceptance(const char *, const char *, const char *);
extern void nmb_acceptance_describe(void);

static int primed(const char *path) {
    char boot[64] = {0}, token[80], *line = NULL;
    size_t size = sizeof boot, capacity = 0;
    if (sysctlbyname("kern.bootsessionuuid", boot, &size, NULL, 0)) return 0;
    boot[sizeof boot - 1] = 0;
    snprintf(token, sizeof token, " boot=%s ", boot);
    FILE *file = fopen(path, "r");
    if (!file) return 0;
    int region = 0, complete = 0;
    while (getline(&line, &capacity, file) >= 0) {
        if (!strncmp(line, "list ", 5)) {
            region = strstr(line, token) && strstr(line, " scope=2 ");
            complete = 0;
        } else if (!strncmp(line, "pass ", 5)) {
            complete = region && strstr(line, token) && strstr(line, " scope=2 ") && strstr(line, " complete=1 ");
        }
    }
    free(line);
    fclose(file);
    if (complete) setenv("NATIVE_METAL_BOOT_SESSION", boot, 1);
    return complete;
}

int main(int argc, char **argv) {
    const char *loader = NULL, *cache = NULL, *prime = NULL, *mode = "all";
    if (argc == 2 && !strcmp(argv[1], "--describe")) {nmb_acceptance_describe(); return 0;}
    for (int i = 1; i < argc; ++i) {
        if (i + 1 >= argc) goto usage;
        if (!strcmp(argv[i], "--loader")) loader = argv[++i];
        else if (!strcmp(argv[i], "--cache")) cache = argv[++i];
        else if (!strcmp(argv[i], "--prime-log")) prime = argv[++i];
        else if (!strcmp(argv[i], "--mode")) mode = argv[++i];
        else goto usage;
    }
    if (!loader || !cache || !prime) goto usage;
    if (strcmp(mode, "all") && strcmp(mode, "numeric") && strcmp(mode, "calibrate")) goto usage;
    if (!primed(prime)) {
        fprintf(stderr, "Current boot whole-system shader priming is incomplete: %s\n", prime);
        return 69;
    }
    if (!strcmp(mode, "all")) {
        int result = nmb_acceptance(loader, cache, "numeric");
        if (result) return result;
        return nmb_acceptance(loader, cache, "calibrate");
    }
    return nmb_acceptance(loader, cache, mode);
usage:
    fprintf(stderr, "gpu-acceptance --loader PATH --cache DIR --prime-log PATH [--mode numeric|calibrate|all]\n");
    return 64;
}
