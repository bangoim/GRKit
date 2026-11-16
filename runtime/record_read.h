#ifndef GRKIT_RECORD_READ_H
#define GRKIT_RECORD_READ_H
/* Enter before loading any registry pointer. The publisher switches banks,
   then waits for a zero-reader grace point before reusing the retired bank.
   Overflow/underflow saturates and prevents reuse instead of freeing early. */
static __attribute__((unused)) void record_read_change(volatile u32 *h,u32 delta) {
    if(!h[GRKIT_HOST_TAIL/4]) return;
    volatile u32 *word=&WORD(h[GRKIT_HOST_TAIL/4],GRKIT_GLOBAL_RECORD_READERS);
    u32 before=__atomic_load_n(word,__ATOMIC_SEQ_CST);
    while(before!=0xffffffffU &&
          !__atomic_compare_exchange_n(word,&before,before+delta,0,__ATOMIC_SEQ_CST,__ATOMIC_SEQ_CST)) {}
}
#define record_read_enter(h) record_read_change((h),1U)
#define record_read_leave(h) record_read_change((h),0xffffffffU)
#endif
