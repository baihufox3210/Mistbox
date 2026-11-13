/** The same selectable mark is used by every brand header. */
export function Logo({ size = 24 }: { size?: number }) {
  return (
    <span className="logo" style={{ width: size, height: size }} aria-hidden="true">
      <img className="logo-dark" src="/logo-dark.svg" alt="" width={size} height={size} />
      <img className="logo-light" src="/logo-light.svg" alt="" width={size} height={size} />
    </span>
  );
}
