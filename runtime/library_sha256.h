#ifndef GRKIT_LIBRARY_SHA256_H
#define GRKIT_LIBRARY_SHA256_H
/* Bounded SHA-256 for immutable GRF/manifest and snapshot verification. The caller
   supplies u8/u32. No allocation, firmware service, or shared state is used. */
static u32 library_sha256_rotr(u32 value,u32 bits)
{ return (value>>bits)|(value<<(32-bits)); }
static __attribute__((noinline)) void library_sha256_block(u32 state[8],const u8 block[64])
{
    static const u32 constants[64]={
        0x428a2f98U,0x71374491U,0xb5c0fbcfU,0xe9b5dba5U,0x3956c25bU,0x59f111f1U,0x923f82a4U,0xab1c5ed5U,
        0xd807aa98U,0x12835b01U,0x243185beU,0x550c7dc3U,0x72be5d74U,0x80deb1feU,0x9bdc06a7U,0xc19bf174U,
        0xe49b69c1U,0xefbe4786U,0x0fc19dc6U,0x240ca1ccU,0x2de92c6fU,0x4a7484aaU,0x5cb0a9dcU,0x76f988daU,
        0x983e5152U,0xa831c66dU,0xb00327c8U,0xbf597fc7U,0xc6e00bf3U,0xd5a79147U,0x06ca6351U,0x14292967U,
        0x27b70a85U,0x2e1b2138U,0x4d2c6dfcU,0x53380d13U,0x650a7354U,0x766a0abbU,0x81c2c92eU,0x92722c85U,
        0xa2bfe8a1U,0xa81a664bU,0xc24b8b70U,0xc76c51a3U,0xd192e819U,0xd6990624U,0xf40e3585U,0x106aa070U,
        0x19a4c116U,0x1e376c08U,0x2748774cU,0x34b0bcb5U,0x391c0cb3U,0x4ed8aa4aU,0x5b9cca4fU,0x682e6ff3U,
        0x748f82eeU,0x78a5636fU,0x84c87814U,0x8cc70208U,0x90befffaU,0xa4506cebU,0xbef9a3f7U,0xc67178f2U
    };
    /* Only the previous sixteen schedule words are needed. Keep the resident
       verifier within the runtime's 256-byte per-function stack budget. */
    u32 words[16];
    for(u32 i=0;i<16;++i) words[i]=((u32)block[4*i]<<24)|((u32)block[4*i+1]<<16)|((u32)block[4*i+2]<<8)|block[4*i+3];
    u32 a=state[0],b=state[1],c=state[2],d=state[3],e=state[4],f=state[5],g=state[6],h=state[7];
    for(u32 i=0;i<64;++i) {
        if(i>=16) {
            u32 x=words[(i-15)&15U],y=words[(i-2)&15U];
            u32 small0=library_sha256_rotr(x,7)^library_sha256_rotr(x,18)^(x>>3);
            u32 small1=library_sha256_rotr(y,17)^library_sha256_rotr(y,19)^(y>>10);
            words[i&15U]+=small0+words[(i-7)&15U]+small1;
        }
        u32 s1=library_sha256_rotr(e,6)^library_sha256_rotr(e,11)^library_sha256_rotr(e,25);
        u32 first=h+s1+((e&f)^((~e)&g))+constants[i]+words[i&15U];
        u32 s0=library_sha256_rotr(a,2)^library_sha256_rotr(a,13)^library_sha256_rotr(a,22);
        u32 second=s0+((a&b)^(a&c)^(b&c));
        h=g; g=f; f=e; e=d+first; d=c; c=b; b=a; a=first+second;
    }
    state[0]+=a; state[1]+=b; state[2]+=c; state[3]+=d;
    state[4]+=e; state[5]+=f; state[6]+=g; state[7]+=h;
}
#define LIBRARY_SHA256_MAX_BYTES (1024U*1024U)
struct library_sha256_state { u32 words[8],length,used; u8 tail[64]; };
static void library_sha256_init(struct library_sha256_state *hash)
{
    static const u32 initial[8]={0x6a09e667U,0xbb67ae85U,0x3c6ef372U,0xa54ff53aU,0x510e527fU,0x9b05688cU,0x1f83d9abU,0x5be0cd19U};
    for(u32 i=0;i<8;++i) hash->words[i]=initial[i];
    hash->length=hash->used=0;
}
static int library_sha256_update(struct library_sha256_state *hash,const u8 *bytes,u32 length)
{
    if(!hash || (!bytes && length) || hash->length>LIBRARY_SHA256_MAX_BYTES ||
       length>LIBRARY_SHA256_MAX_BYTES-hash->length || hash->used!=hash->length%64U) return 0;
    hash->length+=length;
    if(hash->used) {
        while(length && hash->used<64) { hash->tail[hash->used++]=*bytes++; --length; }
        if(hash->used==64) { library_sha256_block(hash->words,hash->tail); hash->used=0; }
    }
    while(length>=64) { library_sha256_block(hash->words,bytes); bytes+=64; length-=64; }
    while(length--) hash->tail[hash->used++]=*bytes++;
    return 1;
}
/* Finalization copies the state so retries can read the same pinned digest. */
static int library_sha256_final(const struct library_sha256_state *hash,u8 digest[32])
{
    if(!hash || !digest || hash->length>LIBRARY_SHA256_MAX_BYTES || hash->used!=hash->length%64U) return 0;
    u32 state[8]; u8 tail[64]={0};
    for(u32 i=0;i<8;++i) state[i]=hash->words[i];
    for(u32 i=0;i<hash->used;++i) tail[i]=hash->tail[i];
    tail[hash->used]=0x80;
    if(hash->used>=56) {
        library_sha256_block(state,tail);
        for(u32 i=0;i<64;++i) tail[i]=0;
    }
    u32 bits=hash->length*8;
    for(u32 i=0;i<4;++i) tail[63-i]=(u8)(bits>>(i*8));
    library_sha256_block(state,tail);
    for(u32 i=0;i<32;++i) digest[i]=(u8)(state[i/4]>>(24-8*(i%4)));
    return 1;
}
static __attribute__((noinline)) int library_sha256(const u8 *bytes,u32 length,u8 digest[32])
{
    if(length>32768U) return 0;
    struct library_sha256_state hash;
    library_sha256_init(&hash);
    return library_sha256_update(&hash,bytes,length) && library_sha256_final(&hash,digest);
}
#endif
