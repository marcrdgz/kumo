import { RootProvider } from 'fumadocs-ui/provider/next';
import { Inter, JetBrains_Mono, Martian_Mono } from 'next/font/google';
import './global.css';

const inter = Inter({
  subsets: ['latin'],
  variable: '--font-inter',
});

// Display face for landing headlines: terminal-native, wide and deliberate.
const display = Martian_Mono({
  subsets: ['latin'],
  variable: '--font-martian-mono',
});

const mono = JetBrains_Mono({
  subsets: ['latin'],
  variable: '--font-jetbrains-mono',
});

export default function Layout({ children }: LayoutProps<'/'>) {
  return (
    <html
      lang="en"
      className={`${inter.variable} ${mono.variable} ${display.variable}`}
      suppressHydrationWarning
    >
      <body className="flex flex-col min-h-screen">
        <RootProvider
          search={{
            options: {
              type: 'static',
              api: `${process.env.NEXT_PUBLIC_BASE_PATH ?? ''}/api/search`,
            },
          }}
        >
          {children}
        </RootProvider>
      </body>
    </html>
  );
}
