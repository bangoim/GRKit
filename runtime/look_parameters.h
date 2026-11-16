#ifndef GRKIT_LOOK_PARAMETERS_H
#define GRKIT_LOOK_PARAMETERS_H
/* Stable identity for Fn/Toning, independent of native ImageTone remapping.
   Full ASCII IDs accompany every persisted hash bucket; hash equality alone
   never authorizes restoring parameters from a previous catalog. */
static __attribute__((unused)) u32 look_parameter_code(const volatile u8 *id) {
    u32 hash=0x811c9dc5U;
    for(u32 i=0;i<32 && id[i];++i) hash=(hash^id[i])*0x01000193U;
    return hash&0x7fffU;
}
static __attribute__((unused)) u32 look_parameter_id_word(const volatile u8 *id,u32 word) {
    u32 value=0;
    for(u32 i=0;i<4;++i) value|=(u32)id[word*4+i]<<(i*7U);
    return value;
}
#endif
