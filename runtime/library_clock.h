#ifndef GRKIT_LIBRARY_CLOCK_H
#define GRKIT_LIBRARY_CLOCK_H
/* Divide a 64-bit architectural counter without linking __aeabi_uldivmod.
   The low 32 bits of milliseconds wrap naturally for bounded TTL subtraction. */
static int library_clock_ms_parts(u32 high,u32 low,u32 ticks_per_ms,u32 *milliseconds)
{
    if(!ticks_per_ms || ticks_per_ms>0x7fffffffU || !milliseconds) return 0;
    u32 remainder=0,result=0;
    for(u32 word=0;word<2;++word) {
        u32 input=word?low:high;
        for(u32 bit=0;bit<32;++bit) {
            remainder=(remainder<<1)|(input>>31); input<<=1;
            u32 one=remainder>=ticks_per_ms;
            if(one) remainder-=ticks_per_ms;
            result=(result<<1)|one;
        }
    }
    *milliseconds=result; return 1;
}
static int library_clock_ms(u32 *milliseconds)
{
#if defined(__arm__)
    u32 ticks_per_ms;
#ifdef SYSTEM_COUNTER_MHZ
    ticks_per_ms=SYSTEM_COUNTER_MHZ*1000U;
#else
    /* Use the architectural frequency on targets without a verified constant.
       An unset or fractional millisecond rate cannot establish a pin deadline. */
    u32 frequency;
    __asm__ volatile("mrc p15, 0, %0, c14, c0, 0" : "=r"(frequency));
    if(!library_clock_ms_parts(0,frequency,1000U,&ticks_per_ms) ||
       !ticks_per_ms || ticks_per_ms*1000U!=frequency) return 0;
#endif
    u32 low,high;
    __asm__ volatile("isb\n\tmrrc p15, 0, %0, %1, c14" : "=r"(low), "=r"(high) : : "memory");
    return library_clock_ms_parts(high,low,ticks_per_ms,milliseconds);
#else
    (void)milliseconds; return 0;
#endif
}
#endif
