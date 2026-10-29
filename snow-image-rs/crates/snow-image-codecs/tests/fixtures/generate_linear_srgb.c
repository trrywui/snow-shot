/* GPL-3.0-or-later. Generates our deterministic linear-sRGB ICC test profile. */
#include <lcms2.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
int main(int argc, char **argv) {
    if (argc != 2) return 1;
    cmsCIExyY white = {0.3127, 0.3290, 1.0};
    cmsCIExyYTRIPLE primaries = {{0.64,0.33,1.0},{0.30,0.60,1.0},{0.15,0.06,1.0}};
    cmsToneCurve *gamma = cmsBuildGamma(NULL, 1.0);
    cmsToneCurve *curves[] = {gamma, gamma, gamma};
    cmsHPROFILE profile = cmsCreateRGBProfile(&white, &primaries, curves);
    cmsUInt32Number length = 0;
    if (!profile || !cmsSaveProfileToMem(profile, NULL, &length)) return 2;
    uint8_t *bytes = malloc(length);
    if (!bytes || !cmsSaveProfileToMem(profile, bytes, &length)) return 3;
    /* ICC dateTimeNumber: 2026-01-01 00:00:00, independent of generator clock. */
    const uint8_t date[] = {7,234,0,1,0,1,0,0,0,0,0,0};
    for (size_t i=0;i<sizeof(date);++i) bytes[24+i]=date[i];
    FILE *file = fopen(argv[1], "wb");
    if (!file) return 4;
    int okay = fwrite(bytes, 1, length, file) == length;
    fclose(file); free(bytes); cmsCloseProfile(profile); cmsFreeToneCurve(gamma);
    return okay ? 0 : 5;
}
