import { useEffect, useRef, useState } from "react";

export function MarqueeText({ text }: { text: string }) {
  const outerRef = useRef<HTMLDivElement>(null);
  const innerRef = useRef<HTMLSpanElement>(null);
  const [scrolling, setScrolling] = useState(false);

  useEffect(() => {
    const outer = outerRef.current;
    const inner = innerRef.current;
    if (outer && inner) setScrolling(inner.scrollWidth > outer.clientWidth + 2);
  }, [text]);

  return (
    <div ref={outerRef} className={scrolling ? "marquee scrolling" : "marquee"} title={text}>
      <span ref={innerRef} className="marquee-inner">{text}</span>
      {scrolling && <span className="marquee-inner" aria-hidden="true">{text}</span>}
    </div>
  );
}
