import { useState } from "react";

interface Props {
  text: string;
  className?: string;
}

export function CopyCmd({ text, className = "lp-cmd-block" }: Props) {
  const [copied, setCopied] = useState(false);

  const onCopy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      setTimeout(() => setCopied(false), 1600);
    } catch {
      /* clipboard denied */
    }
  };

  return (
    <div className={className}>
      <pre className="lp-cmd">{text}</pre>
      <button
        type="button"
        className="lp-cmd-copy"
        onClick={onCopy}
        aria-label="Copy command"
      >
        {copied ? "copied" : "copy"}
      </button>
    </div>
  );
}
