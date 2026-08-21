/* EFR32MG24 (xG24) class part: 1536 KB flash, 256 KB RAM.
 *
 * These are region sizes for the linker, not a claim about a specific SKU.
 * The measurement this harness produces is a section-size delta, which does
 * not depend on the region sizes at all: they only need to be large enough
 * that the link succeeds. */
MEMORY
{
  FLASH : ORIGIN = 0x08000000, LENGTH = 1536K
  RAM   : ORIGIN = 0x20000000, LENGTH = 256K
}
