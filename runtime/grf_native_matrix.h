#ifndef GRKIT_GRF_NATIVE_MATRIX_H
#define GRKIT_GRF_NATIVE_MATRIX_H
#include "abi.h"
static u32 grf_matrix_get16(const u8 *p) { return p[0]|((u32)p[1]<<8); }
/* floor(x / 1024), also for a negative x. */
static int grf_floor1024(int x) { return x>=0 ? x>>10 : -((-x+1023)>>10); }
/* The Q10 matrix m (row-major) of a bank at a level of a native look
   (grkit_core::native_look::NativeLook::level_matrix): each off-diagonal is the
   level-0 coefficient times the level's gain, rounded, plus the level's
   correction; the diagonal is the gain minus the row's off-diagonals. */
static void grf_native_matrix(const u8 *p, u32 bank, u32 level, int *m)
{
    int gain=(int)grf_matrix_get16(p+GRKIT_NATIVE_GAINS+level*2);
    for (u32 r=0; r<3; ++r) {
        int sum=0;
        for (u32 j=0; j<2; ++j) {
            int c=(short)grf_matrix_get16(p+GRKIT_NATIVE_MATRICES+(bank*6+r*2+j)*2);
            int v=grf_floor1024(c*gain+512);
            if (level) v+=(signed char)p[GRKIT_NATIVE_CORRECTIONS+((level-1)*12+bank)*6+r*2+j];
            u32 column=r+1+j;
            if(column>=3) column-=3;
            m[r*3+column]=v;
            sum+=v;
        }
        m[r*4]=gain-sum;
    }
}
#endif
