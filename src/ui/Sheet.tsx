import React from 'react';
import { motion } from 'motion/react';

export function Sheet({ children, onDismiss }: { children: React.ReactNode; onDismiss: () => void }) {
  return (
    <motion.div
      initial={{ y: 40, opacity: 0 }}
      animate={{ y: 0, opacity: 1 }}
      exit={{ y: 40, opacity: 0 }}
      transition={{ duration: 0.26, ease: [0.32, 0.72, 0, 1] }}
      drag="y"
      dragConstraints={{ top: 0 }}
      dragElastic={0.2}
      onDragEnd={(_, info) => {
        if (info.velocity.y > 0.11 || info.offset.y > 200) onDismiss();
      }}
      style={{
        position: 'fixed',
        bottom: 0,
        left: '50%',
        x: '-50%',
        width: 720,
        maxWidth: '94vw',
        height: '70vh',
        background: 'var(--bg-elevated)',
        boxShadow: 'var(--shadow-sheet)',
        borderRadius: '12px 12px 0 0',
        zIndex: 50,
        display: 'flex',
        flexDirection: 'column',
      }}
    >
      {children}
    </motion.div>
  );
}
