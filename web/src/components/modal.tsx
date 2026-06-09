import { useEffect, useRef } from "react";
import type { ReactNode } from "react";

interface ModalProps {
  open: boolean;
  onClose: () => void;
  /**
   * If false, Escape does NOT close this modal. Used to implement
   * nested modals: the outer modal passes `closeOnEscape={false}` while
   * the inner one is open, so Escape only closes the inner one (the
   * inner modal's own listener fires second but the outer one's
   * `useEffect` already returned early on the same event).
   * Default: true.
   */
  closeOnEscape?: boolean;
  /**
   * Stack order for nested modals. Default: 50. Pass a higher value
   * (e.g. 60) for a modal rendered inside another so it visually
   * sits on top of the outer's backdrop.
   */
  zIndex?: number;
  children: ReactNode;
}

export function Modal({ open, onClose, closeOnEscape = true, zIndex = 50, children }: ModalProps) {
  const backdropRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open || !closeOnEscape) return;

    const handleEscape = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };

    document.addEventListener("keydown", handleEscape);
    return () => document.removeEventListener("keydown", handleEscape);
  }, [open, onClose, closeOnEscape]);

  if (!open) return null;

  return (
    <div
      ref={backdropRef}
      style={{ zIndex }}
      className="fixed inset-0 flex items-center justify-center bg-black/50 backdrop-blur-sm"
      onClick={(e) => {
        if (e.target === backdropRef.current) onClose();
      }}
    >
      <div className="bg-white rounded-2xl shadow-xl max-w-lg w-full mx-4 p-6 animate-fade-in">
        {children}
      </div>
    </div>
  );
}
