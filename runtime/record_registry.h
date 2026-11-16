#ifndef GRKIT_RECORD_REGISTRY_H
#define GRKIT_RECORD_REGISTRY_H
/* Logical slots stay stable. The App publishes one immutable bank per slot. */
static __attribute__((unused)) u32 record_address(volatile u32 *h,u32 index) {
    u32 bank=(h[GRKIT_HOST_RECORD_BANK_BITS/4+(index>>5)]>>(index&31))&1U;
    return (bank ? h[GRKIT_HOST_RECORD_BANK2/4] : h[GRKIT_HOST_RECORDS/4])+index*GRKIT_RECORD_SIZE;
}
#endif
