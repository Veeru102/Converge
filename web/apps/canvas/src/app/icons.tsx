const I = ({ children, ...rest }: React.SVGProps<SVGSVGElement>) => (
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} strokeLinecap="round" strokeLinejoin="round" aria-hidden {...rest}>{children}</svg>
);
export const Icons = {
  cursor: () => <I><path d="M5 3l14 8.5-6.5 1.5L10 20z" /></I>,
  rect: () => <I><rect x={4} y={5} width={16} height={14} rx={2} /></I>,
  ellipse: () => <I><ellipse cx={12} cy={12} rx={8} ry={6.5} /></I>,
  line: () => <I><path d="M5 19L19 5" /><path d="M12 5h7v7" /></I>,
  text: () => <I><path d="M5 6h14M12 6v13M9 19h6" /></I>,
  copy: () => <I><rect x={9} y={9} width={11} height={11} rx={2} /><path d="M5 15V6a2 2 0 012-2h9" /></I>,
  trash: () => <I><path d="M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3" /></I>,
  front: () => <I><path d="M12 4l7 4-7 4-7-4z" /><path d="M5 12l7 4 7-4M5 16l7 4 7-4" opacity={0.5} /></I>,
  back: () => <I><path d="M12 12l7 4-7 4-7-4z" /><path d="M5 8l7-4 7 4M5 12l7-4 7 4" opacity={0.5} /></I>,
  up: () => <I><path d="M6 15l6-6 6 6" /></I>,
  down: () => <I><path d="M6 9l6 6 6-6" /></I>,
  plus: () => <I><path d="M12 5v14M5 12h14" /></I>,
  minus: () => <I><path d="M5 12h14" /></I>,
  fit: () => <I><path d="M4 9V5h4M20 9V5h-4M4 15v4h4M20 15v4h-4" /></I>,
  lab: () => <I><path d="M9 3h6M10 3v6l-5.5 9A2 2 0 006.2 21h11.6a2 2 0 001.7-3L14 9V3" /><path d="M8 15h8" /></I>,
  close: () => <I><path d="M6 6l12 12M18 6L6 18" /></I>,
  check: () => <I><path d="M5 12l5 5L20 7" /></I>,
  keyboard: () => <I><rect x={3} y={6} width={18} height={12} rx={2} /><path d="M7 10h.01M11 10h.01M15 10h.01M7 14h10" /></I>,
  logo: () => <svg viewBox="0 0 24 24" width={14} height={14} fill="currentColor" aria-hidden><circle cx={8} cy={12} r={5} opacity={0.9} /><circle cx={16} cy={12} r={5} opacity={0.6} /></svg>,
  wifiOff: () => <I><path d="M2 8.5a16 16 0 0120 0M5 12a11 11 0 0114 0M8.5 15.5a6 6 0 017 0M12 19h.01M3 3l18 18" /></I>,
  refresh: () => <I><path d="M20 12a8 8 0 01-14.5 4.6M4 12a8 8 0 0114.5-4.6M20 4v4h-4M4 20v-4h4" /></I>,
};
export type IconName = keyof typeof Icons;
