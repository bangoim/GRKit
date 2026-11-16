#include <assert.h>
#include <stdint.h>
typedef uint32_t u32;
#include "library_clock.h"
int main(void)
{
    u32 result,seed=1;
    const u32 frequencies[]={1000,24000,40000,1000000};
    for(u32 i=0;i<10000;++i) {
        seed=seed*1664525U+1013904223U; u32 high=seed;
        seed=seed*1664525U+1013904223U; u32 low=seed;
        for(u32 j=0;j<4;++j) {
            assert(library_clock_ms_parts(high,low,frequencies[j],&result));
            assert(result==(u32)((((uint64_t)high<<32)|low)/frequencies[j]));
        }
    }
    assert(library_clock_ms_parts(0,0xffffffffU,40000,&result) && result==107374);
    assert(library_clock_ms_parts(1,0,40000,&result) && result==107374);
    uint64_t ticks=((uint64_t)0xffffffffU+10)*40000;
    assert(library_clock_ms_parts((u32)(ticks>>32),(u32)ticks,40000,&result) && result==9);
    assert(!library_clock_ms_parts(0,0,0,&result));
    assert(!library_clock_ms_parts(0,0,40000,0));
    assert(!library_clock_ms(&result)); /* Host cannot impersonate the camera timer. */
    return 0;
}
