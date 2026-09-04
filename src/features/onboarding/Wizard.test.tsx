import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { api } from '../../app/ipc/commands';
import { useSetup } from '../../stores/setupStore';
import { StepConnecting } from './StepConnecting';
import { Wizard } from './Wizard';

vi.mock('../../app/ipc/commands', () => ({
  api: {
    system_info: vi.fn().mockResolvedValue({ oauth_available: false }),
    accounts_probe_email: vi.fn().mockResolvedValue(true),
    accounts_add_app_password: vi.fn().mockResolvedValue({ id: 'a1' }),
    app_open_url: vi.fn().mockResolvedValue(undefined),
  },
}));

function reset(step: 'welcome' | 'email' | 'app-password' | 'connecting' = 'welcome') {
  useSetup.setState({
    step,
    email: step === 'welcome' ? '' : 'you@gmail.com',
    appPassword: '',
    oauthAvailable: false,
    progress: null,
    googleHosted: null,
  });
}

describe('P11-T17 wizard order and keyboard', () => {
  it('renders welcome first with Connect your Gmail', async () => {
    reset('welcome');
    render(<Wizard onDone={() => undefined} />);
    expect(await screen.findByText('Connect your Gmail')).toBeInTheDocument();
    expect(screen.queryByText('Sign in with Google instead')).not.toBeInTheDocument();
  });

  it('shows Google button only when oauth_available', async () => {
    reset('welcome');
    vi.mocked(api.system_info).mockResolvedValueOnce({
      version: '1.1.0',
      oauth_available: true,
      demo: false,
    });
    render(<Wizard onDone={() => undefined} />);
    expect(await screen.findByText('Sign in with Google instead')).toBeInTheDocument();
  });

  it('Enter advances email, Esc goes back, first field focused', async () => {
    reset('email');
    const user = userEvent.setup();
    render(<Wizard onDone={() => undefined} />);
    const input = screen.getByPlaceholderText('you@gmail.com');
    expect(document.activeElement).toBe(input);
    await user.clear(input);
    await user.type(input, 'you@gmail.com');
    await user.keyboard('{Enter}');
    expect(await screen.findByText('Get an app password')).toBeInTheDocument();
    await user.keyboard('{Escape}');
    expect(await screen.findByPlaceholderText('you@gmail.com')).toBeInTheDocument();
  });

  it('Connect disabled until valid; disclosure toggles', async () => {
    reset('app-password');
    const user = userEvent.setup();
    render(<Wizard onDone={() => undefined} />);
    const connect = screen.getByText('Connect') as HTMLButtonElement;
    expect(connect.disabled).toBe(true);
    const input = screen.getByLabelText('16-letter app password');
    await user.type(input, 'abcd efgh ijkl mnop');
    expect((screen.getByText('Connect') as HTMLButtonElement).disabled).toBe(false);
    await user.click(screen.getByText('Why an app password?'));
    expect(await screen.findByText(/Mac's Keychain/)).toBeInTheDocument();
  });
});

describe('P11-T18 connecting rows and errors', () => {
  it('ticks rows through syncing and shows fix copy on error', async () => {
    vi.mocked(api.accounts_add_app_password).mockImplementationOnce(() => new Promise(() => {}));
    useSetup.setState({
      step: 'connecting',
      email: 'you@gmail.com',
      appPassword: 'abcdefghijklmnop',
      oauthAvailable: false,
      progress: 'listing',
      googleHosted: true,
    });
    render(<StepConnecting onDone={() => undefined} />);
    expect(screen.getByText('Reading your labels')).toBeInTheDocument();
    useSetup.setState({ progress: 'error:imap_bad_password' });
    expect(
      await screen.findByText("That app password didn't work. Create a fresh one and paste it again."),
    ).toBeInTheDocument();
    expect(screen.getByText('Back to app password')).toBeInTheDocument();
  });
});
