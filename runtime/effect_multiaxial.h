#ifndef GRKIT_EFFECT_MULTIAXIAL_H
#define GRKIT_EFFECT_MULTIAXIAL_H
#include "effect_record.h"
#include "grf_native_matrix.h"
#define EFFECT_AT(a) (out+((a)-record))
#define EFFECT_STORE(a,v) effect_record_store(EFFECT_AT(a),(v))
#define EFFECT_WORD(a) effect_record_word(EFFECT_AT(a))
static const unsigned short multiaxial_bounds[16] = GRKIT_HUE_BOUNDS;
static const u8 multiaxial_banks[20] = GRKIT_HUE_SELECTORS;
static const unsigned short multiaxial_levels[8] = GRKIT_HUE_LEVEL_CONTROL;
static const unsigned short multiaxial_levels_halved[8] = GRKIT_HUE_LEVEL_CONTROL_HALVED;
static void effect_record_multiaxial(u8 *out,u32 record,const u8 *profile,u32 regions,u32 rgb)
{
    const u8 *sectors = profile + GRKIT_GRF_HUE;
    int levels = (effect_record_word(profile + GRKIT_GRF_FEATURES) & GRKIT_FEATURE_HUE_LEVELS) != 0;
    int native = (effect_record_word(profile + GRKIT_GRF_FEATURES) & GRKIT_FEATURE_NATIVE_LOOK) != 0;
    u32 slot = record + GRKIT_RECORD_MULTIAXIAL, table = slot + 0x98;
    EFFECT_STORE(slot, slot + 8);
    EFFECT_STORE(slot + 8, slot + 0x20);
    EFFECT_STORE(slot + 12, slot + 0x40);
    EFFECT_STORE(slot + 16, slot + 0x58);
    EFFECT_STORE(slot + 20, regions);
    EFFECT_STORE(slot + 24, slot + 0x6c);
    EFFECT_STORE(slot + 28, rgb);
    effect_record_copy(EFFECT_AT(slot + 0x20), native ? profile + GRKIT_NATIVE_BOUNDS : (const u8 *)multiaxial_bounds, sizeof multiaxial_bounds);
    effect_record_copy(EFFECT_AT(slot + 0x40), native ? profile + GRKIT_NATIVE_SELECTORS : multiaxial_banks, sizeof multiaxial_banks);
    for (u32 k = 0; k < 5; ++k) EFFECT_STORE(slot + 0x58 + k * 4, table);
    volatile unsigned short *control = (volatile unsigned short *)EFFECT_AT(slot + 0x6c);
    if (native) {
        const u8 *fields = profile + GRKIT_NATIVE_CONTROL;
        for (u32 k = 0; k < 21; ++k) control[k] = (unsigned short)(fields[k * 2] | fields[k * 2 + 1] << 8);
        for (u32 bank = 0; bank < 12; ++bank)
            for (u32 level = 0; level < 5; ++level) {
                int m[9];
                grf_native_matrix(profile, bank, level, m);
                for (u32 k = 0; k < 9; ++k) EFFECT_STORE(table + (bank * 5 + level) * 36 + k * 4, (u32)m[k]);
            }
        EFFECT_STORE(slot + 4, slot + 8);
        return;
    }
    control[0] = 0;
    for (u32 k = 1; k <= 8; ++k) control[k] = levels ? multiaxial_levels[k - 1] : 0xfff;
    control[9] = 0;
    const u8 *fields = sectors + GRKIT_HUE_MATRICES;
    for (u32 k = 0; k < 11; ++k) control[10 + k] = (unsigned short)(fields[k * 2] | fields[k * 2 + 1] << 8);
    for (u32 bank = 0; bank < 12; ++bank)
        for (u32 level = 0; level < 5; ++level) {
            /* Levels 1 to 3 follow GRKIT_HUE_LEVEL_OFFSET; level 4 repeats level 3. */
            u32 from = levels ? (level < 4 ? level : 3) : 0;
            const u8 *c = from ? profile + GRKIT_HUE_LEVEL_OFFSET + (from - 1) * GRKIT_HUE_MATRICES : sectors;
            int m[9];
            for(u32 k=0;k<9;++k) ((volatile int *)m)[k]=0;
            m[0]=m[4]=m[8]=1024;
            for (u32 r = 0; bank && r < 3; ++r) {
                /* GRF validation keeps each diagonal within the 12-bit register. */
                int a = (signed char)c[(bank - 1) * 6 + r * 2] * 8;
                int b = (signed char)c[(bank - 1) * 6 + r * 2 + 1] * 8;
                m[r * 3 + (r==2 ? 0 : r+1)] = a;
                m[r * 3 + (r ? r-1 : 2)] = b;
                m[r * 4] = 1024 - a - b;
            }
            for (u32 k = 0; k < 9; ++k) EFFECT_STORE(table + (bank * 5 + level) * 36 + k * 4, (u32)m[k]);
        }
    /* The set for a capture with highlight correction, which the hooks pick by the
       native flag. The camera then halves the stage's input, so these control fields
       select the levels and the weight of twice the luminance: the weight breakpoints
       halve and its slopes double (9-bit, clamped). The address at +4, the set at
       +0x908, its control fields at +0x920; the other pointers are those of the set. */
    u32 halved = slot + 0x908;
    EFFECT_STORE(slot + 4, halved);
    for (u32 k = 0; k < 6; ++k) EFFECT_STORE(halved + k * 4, EFFECT_WORD(slot + 8 + k * 4));
    EFFECT_STORE(halved + 16, slot + 0x920);
    volatile unsigned short *half = (volatile unsigned short *)EFFECT_AT(slot + 0x920);
    half[0] = 0;
    for (u32 k = 1; k <= 8; ++k) half[k] = levels ? multiaxial_levels_halved[k - 1] : 0xfff;
    half[9] = 0;
    for (u32 k = 0; k < 4; ++k) {
        int slope = (int)(control[14 + k] & 0x1ff) - ((control[14 + k] & 0x100) ? 0x200 : 0);
        slope *= 2;
        if (slope > 255) slope = 255;
        if (slope < -256) slope = -256;
        half[10 + k] = control[10 + k];
        half[14 + k] = (unsigned short)(slope & 0x1ff);
    }
    for (u32 k = 18; k <= 20; ++k) half[k] = (unsigned short)(control[k] / 2);
}
#undef EFFECT_AT
#undef EFFECT_STORE
#undef EFFECT_WORD
#endif
